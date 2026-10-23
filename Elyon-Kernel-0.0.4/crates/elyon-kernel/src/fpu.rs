//! K004-06 : activation FPU/SSE/AVX et sauvegarde du contexte SIMD par tâche.
//!
//! Le noyau reste compilé en soft-float : seul le code des tâches (assembleur)
//! touche aux registres x87/SSE/AVX. Le scheduler sauvegarde l'état de la tâche
//! sortante et restaure celui de la tâche entrante à chaque switch (stratégie
//! eager), dans une zone PMM propre à chaque tâche.

use core::arch::asm;
use core::ptr::{copy_nonoverlapping, write_bytes};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::fpu::{self, AreaLayout, CpuFeatures, FpuError, Strategy};

use crate::memory;

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_OSXSAVE: u64 = 1 << 18;

/// Build avec la variable `ELYON_FPU_FORCE_FXSAVE` définie : qualifie le repli
/// FXSAVE sur un CPU qui dispose de XSAVE.
const FORCE_FXSAVE: bool = option_env!("ELYON_FPU_FORCE_FXSAVE").is_some();
static ENABLED: AtomicBool = AtomicBool::new(false);
static USE_XSAVE: AtomicBool = AtomicBool::new(false);
static XCR0: AtomicU64 = AtomicU64::new(0);
static AREA_SIZE: AtomicUsize = AtomicUsize::new(0);
static INIT_AREA: AtomicU64 = AtomicU64::new(0);
static BSP_AREA: AtomicU64 = AtomicU64::new(0);

pub struct FpuSetup {
    pub features: CpuFeatures,
    pub strategy: Strategy,
    pub layout: AreaLayout,
    pub forced_fxsave: bool,
    pub cr0: u64,
    pub cr4: u64,
}

#[derive(Clone, Copy)]
pub enum FpuSetupError {
    Fpu(FpuError),
    AllocationFailed,
    Xcr0Mismatch,
    NotEnabled,
}

impl FpuSetupError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fpu(error) => error.as_str(),
            Self::AllocationFailed => "PMM could not allocate an FPU save area",
            Self::Xcr0Mismatch => "XCR0 does not hold the requested components",
            Self::NotEnabled => "FPU state management is not enabled",
        }
    }
}

impl From<FpuError> for FpuSetupError {
    fn from(error: FpuError) -> Self {
        Self::Fpu(error)
    }
}

pub fn detect() -> CpuFeatures {
    let leaf1 = core::arch::x86_64::__cpuid(1);
    let xsave = leaf1.ecx & (1 << 26) != 0;
    let xcr0_supported = if xsave && core::arch::x86_64::__cpuid(0).eax >= 0xD {
        let leaf = core::arch::x86_64::__cpuid_count(0xD, 0);
        (u64::from(leaf.edx) << 32) | u64::from(leaf.eax)
    } else {
        0
    };
    CpuFeatures {
        fxsr: leaf1.edx & (1 << 24) != 0,
        sse: leaf1.edx & (1 << 25) != 0,
        sse2: leaf1.edx & (1 << 26) != 0,
        xsave,
        avx: leaf1.ecx & (1 << 28) != 0,
        xcr0_supported,
    }
}

/// Active x87/SSE (et AVX via XSAVE si disponible), prépare l'image d'état
/// initial et la zone du BSP. À appeler une fois, sur le BSP, IRQ masquées,
/// avant tout switch de tâche.
pub fn init() -> Result<FpuSetup, FpuSetupError> {
    let features = detect();
    let mut strategy = fpu::choose(&features)?;
    if FORCE_FXSAVE {
        strategy = Strategy::Fxsave;
    }

    let mut cr0 = read_cr0();
    cr0 = (cr0 | CR0_MP) & !(CR0_EM | CR0_TS);
    let mut cr4 = read_cr4() | CR4_OSFXSR | CR4_OSXMMEXCPT;
    if let Strategy::Xsave { .. } = strategy {
        cr4 |= CR4_OSXSAVE;
    }
    // SAFETY: EM=0/MP=1/TS=0 et OSFXSR/OSXMMEXCPT(/OSXSAVE) sont les réglages
    // standard pour exécuter x87/SSE sans piège ; OSXSAVE n'est positionné que si
    // CPUID annonce XSAVE (choose()).
    unsafe {
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }

    let xsave_size = match strategy {
        Strategy::Xsave { xcr0 } => {
            // SAFETY: CR4.OSXSAVE vient d'être positionné et `xcr0` ne contient
            // que des composants rapportés par CPUID.0xD (choose()).
            unsafe { xsetbv(xcr0) };
            // SAFETY: CR4.OSXSAVE est positionné.
            if unsafe { xgetbv() } != xcr0 {
                return Err(FpuSetupError::Xcr0Mismatch);
            }
            XCR0.store(xcr0, Ordering::Release);
            core::arch::x86_64::__cpuid_count(0xD, 0).ebx
        }
        Strategy::Fxsave => 0,
    };

    let layout = fpu::area_layout(strategy, xsave_size)?;
    USE_XSAVE.store(
        matches!(strategy, Strategy::Xsave { .. }),
        Ordering::Release,
    );
    AREA_SIZE.store(layout.size, Ordering::Release);

    let init_area = allocate_area(layout)?;
    // SAFETY: la frame vient d'être réservée au PMM, est mappée en identité et
    // couvre `layout.size` octets (au plus une page, area_layout()).
    let image = unsafe { core::slice::from_raw_parts_mut(init_area as *mut u8, layout.size) };
    fpu::write_init_image(image);
    INIT_AREA.store(init_area, Ordering::Release);
    BSP_AREA.store(allocate_area(layout)?, Ordering::Release);

    ENABLED.store(true, Ordering::Release);
    // SAFETY: `init_area` est une image d'état initial valide pour la stratégie
    // retenue ; le BSP repart d'un état x87/SSE/AVX propre.
    unsafe { restore(init_area) };

    Ok(FpuSetup {
        features,
        strategy,
        layout,
        forced_fxsave: FORCE_FXSAVE,
        cr0: read_cr0(),
        cr4: read_cr4(),
    })
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub fn bsp_area() -> u64 {
    BSP_AREA.load(Ordering::Acquire)
}

/// Zone de sauvegarde d'une tâche neuve, initialisée à l'état propre.
pub fn new_task_area() -> Result<u64, FpuSetupError> {
    if !enabled() {
        return Err(FpuSetupError::NotEnabled);
    }
    let size = AREA_SIZE.load(Ordering::Acquire);
    let layout = AreaLayout {
        size,
        align: if USE_XSAVE.load(Ordering::Acquire) {
            64
        } else {
            16
        },
    };
    let area = allocate_area(layout)?;
    // SAFETY: les deux zones sont mappées, disjointes (frames PMM distinctes) et
    // longues d'au moins `size` octets.
    unsafe {
        copy_nonoverlapping(
            INIT_AREA.load(Ordering::Acquire) as *const u8,
            area as *mut u8,
            size,
        )
    };
    Ok(area)
}

/// Sauvegarde l'état x87/SSE/AVX courant dans `area`.
///
/// # Safety
///
/// `area` doit être une zone obtenue par [`new_task_area`] ou [`bsp_area`],
/// réservée au contexte courant.
pub unsafe fn save(area: u64) {
    if USE_XSAVE.load(Ordering::Acquire) {
        let mask = XCR0.load(Ordering::Acquire);
        // SAFETY: zone alignée sur 64 et de la taille CPUID (contrat de l'appelant).
        unsafe {
            asm!("xsave64 [{}]", in(reg) area, in("eax") mask as u32, in("edx") (mask >> 32) as u32, options(nostack, preserves_flags))
        };
    } else {
        // SAFETY: zone alignée sur 16 et de 512 octets (contrat de l'appelant).
        unsafe { asm!("fxsave64 [{}]", in(reg) area, options(nostack, preserves_flags)) };
    }
}

/// Restaure l'état x87/SSE/AVX depuis `area`.
///
/// # Safety
///
/// `area` doit contenir une image produite par [`save`] ou l'image initiale.
pub unsafe fn restore(area: u64) {
    if USE_XSAVE.load(Ordering::Acquire) {
        let mask = XCR0.load(Ordering::Acquire);
        // SAFETY: image XSAVE valide, zone alignée (contrat de l'appelant).
        unsafe {
            asm!("xrstor64 [{}]", in(reg) area, in("eax") mask as u32, in("edx") (mask >> 32) as u32, options(nostack, preserves_flags))
        };
    } else {
        // SAFETY: image FXSAVE valide, zone alignée (contrat de l'appelant).
        unsafe { asm!("fxrstor64 [{}]", in(reg) area, options(nostack, preserves_flags)) };
    }
}

fn allocate_area(layout: AreaLayout) -> Result<u64, FpuSetupError> {
    let area = memory::allocate_frame().ok_or(FpuSetupError::AllocationFailed)?;
    fpu::validate_area(area, layout)?;
    // SAFETY: frame PMM fraîchement réservée et mappée en identité.
    unsafe { write_bytes(area as *mut u8, 0, PAGE_SIZE as usize) };
    Ok(area)
}

unsafe fn xsetbv(value: u64) {
    // SAFETY: contrat reporté sur l'appelant (CR4.OSXSAVE positionné, composants
    // supportés).
    unsafe {
        asm!("xsetbv", in("ecx") 0u32, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack, preserves_flags))
    };
}

unsafe fn xgetbv() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: contrat reporté sur l'appelant (CR4.OSXSAVE positionné).
    unsafe {
        asm!("xgetbv", in("ecx") 0u32, out("eax") low, out("edx") high, options(nostack, nomem, preserves_flags))
    };
    (u64::from(high) << 32) | u64::from(low)
}

fn read_cr0() -> u64 {
    crate::arch::x86_64::cpu::read_cr0()
}

fn read_cr4() -> u64 {
    crate::arch::x86_64::cpu::read_cr4()
}
