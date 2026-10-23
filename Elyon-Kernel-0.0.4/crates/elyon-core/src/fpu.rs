//! Isolation FPU/SSE/AVX par tâche (K004-06), sans accès matériel.
//!
//! Choisit la stratégie de sauvegarde d'après CPUID, dimensionne la zone de
//! sauvegarde, fournit l'image d'état initial et des motifs SIMD par tâche.

pub const XCR0_X87: u64 = 1 << 0;
pub const XCR0_SSE: u64 = 1 << 1;
pub const XCR0_AVX: u64 = 1 << 2;

/// MXCSR après reset : toutes les exceptions SIMD masquées, arrondi au plus proche.
pub const MXCSR_DEFAULT: u32 = 0x1F80;
/// Mot de contrôle x87 après `fninit`.
pub const FCW_DEFAULT: u16 = 0x037F;

pub const SIMD_REGISTERS: usize = 16;

const FXSAVE_SIZE: usize = 512;
const FXSAVE_ALIGN: usize = 16;
const XSAVE_ALIGN: usize = 64;
/// Zone legacy (512 octets) + en-tête XSAVE (64 octets).
const XSAVE_MIN_SIZE: usize = 576;
/// Une zone par tâche tient dans une frame PMM.
const AREA_MAX_SIZE: usize = 4096;

const FCW_OFFSET: usize = 0;
const MXCSR_OFFSET: usize = 24;

/// Bits CPUID utiles au choix de la stratégie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuFeatures {
    /// CPUID.1:EDX.FXSR[24].
    pub fxsr: bool,
    /// CPUID.1:EDX.SSE[25].
    pub sse: bool,
    /// CPUID.1:EDX.SSE2[26].
    pub sse2: bool,
    /// CPUID.1:ECX.XSAVE[26].
    pub xsave: bool,
    /// CPUID.1:ECX.AVX[28].
    pub avx: bool,
    /// Composants XCR0 supportés, CPUID.(EAX=0xD,ECX=0):EDX:EAX.
    pub xcr0_supported: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// XSAVE/XRSTOR sur les composants `xcr0`, limités à x87|SSE|AVX.
    Xsave { xcr0: u64 },
    /// FXSAVE/FXRSTOR : x87 et SSE seulement.
    Fxsave,
}

impl Strategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xsave { .. } => "xsave",
            Self::Fxsave => "fxsave",
        }
    }

    pub const fn avx(self) -> bool {
        match self {
            Self::Xsave { xcr0 } => xcr0 & XCR0_AVX != 0,
            Self::Fxsave => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpuError {
    NoFxsr,
    NoSse,
    AreaTooSmall,
    AreaTooLarge,
    Misaligned,
}

impl FpuError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFxsr => "CPU lacks FXSAVE/FXRSTOR",
            Self::NoSse => "CPU lacks SSE/SSE2",
            Self::AreaTooSmall => "XSAVE area smaller than legacy region plus header",
            Self::AreaTooLarge => "FPU save area larger than one page",
            Self::Misaligned => "FPU save area misaligned",
        }
    }
}

/// XSAVE avec x87|SSE (et AVX si CPUID et XSAVE le rapportent tous deux), sinon
/// FXSAVE. Les composants au-delà d'AVX ne sont jamais activés.
pub fn choose(features: &CpuFeatures) -> Result<Strategy, FpuError> {
    if !features.fxsr {
        return Err(FpuError::NoFxsr);
    }
    if !features.sse || !features.sse2 {
        return Err(FpuError::NoSse);
    }

    let base = XCR0_X87 | XCR0_SSE;
    if features.xsave && features.xcr0_supported & base == base {
        let mut xcr0 = base;
        if features.avx && features.xcr0_supported & XCR0_AVX != 0 {
            xcr0 |= XCR0_AVX;
        }
        return Ok(Strategy::Xsave { xcr0 });
    }
    Ok(Strategy::Fxsave)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AreaLayout {
    pub size: usize,
    pub align: usize,
}

/// `xsave_size` : CPUID.(EAX=0xD,ECX=0):EBX lu après l'écriture de XCR0.
pub fn area_layout(strategy: Strategy, xsave_size: u32) -> Result<AreaLayout, FpuError> {
    match strategy {
        Strategy::Fxsave => Ok(AreaLayout {
            size: FXSAVE_SIZE,
            align: FXSAVE_ALIGN,
        }),
        Strategy::Xsave { .. } => {
            let size = xsave_size as usize;
            if size < XSAVE_MIN_SIZE {
                return Err(FpuError::AreaTooSmall);
            }
            if size > AREA_MAX_SIZE {
                return Err(FpuError::AreaTooLarge);
            }
            Ok(AreaLayout {
                size,
                align: XSAVE_ALIGN,
            })
        }
    }
}

pub fn validate_area(address: u64, layout: AreaLayout) -> Result<(), FpuError> {
    if address == 0 || !address.is_multiple_of(layout.align as u64) {
        return Err(FpuError::Misaligned);
    }
    Ok(())
}

/// État propre d'une tâche neuve : FCW et MXCSR par défaut, registres et en-tête
/// XSAVE à zéro. Avec XSAVE, XSTATE_BV=0 ramène chaque composant à son état
/// initial au `xrstor` ; MXCSR est relu de la zone legacy dans les deux cas.
pub fn write_init_image(area: &mut [u8]) {
    area.fill(0);
    area[FCW_OFFSET..FCW_OFFSET + 2].copy_from_slice(&FCW_DEFAULT.to_le_bytes());
    area[MXCSR_OFFSET..MXCSR_OFFSET + 4].copy_from_slice(&MXCSR_DEFAULT.to_le_bytes());
}

const fn half(prefix: u64, task: u8, reg: usize) -> u64 {
    prefix
        | ((task as u64) << 40)
        | ((reg as u64 + 1) << 32)
        | 0x5A5A_0000
        | ((task as u64) << 8)
        | reg as u64
}

/// Motif 128 bits de XMM`reg` pour la tâche `task`.
pub const fn xmm_pattern(task: u8, reg: usize) -> u128 {
    ((half(0xF1E6_0000_0000_0000, task, reg) as u128) << 64)
        | half(0xF0E6_0000_0000_0000, task, reg) as u128
}

/// Motif 128 bits de la moitié haute de YMM`reg` pour la tâche `task`.
pub const fn ymm_high_pattern(task: u8, reg: usize) -> u128 {
    ((half(0xA9E6_0000_0000_0000, task, reg) as u128) << 64)
        | half(0xA8E6_0000_0000_0000, task, reg) as u128
}

/// MXCSR propre à la tâche : exceptions masquées, mode d'arrondi (RC, bits
/// 13-14) différent par tâche et toujours distinct de la valeur par défaut.
pub const fn mxcsr_pattern(task: u8) -> u32 {
    MXCSR_DEFAULT | ((((task as u32) % 3) + 1) << 13)
}

/// État SIMD maintenu par une tâche ; disposition lue et écrite par l'assembleur.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimdFile {
    pub xmm: [u128; SIMD_REGISTERS],
    pub ymm_high: [u128; SIMD_REGISTERS],
    pub mxcsr: u32,
}

pub const fn expected_simd(task: u8, avx: bool) -> SimdFile {
    let mut xmm = [0u128; SIMD_REGISTERS];
    let mut ymm_high = [0u128; SIMD_REGISTERS];
    let mut reg = 0;
    while reg < SIMD_REGISTERS {
        xmm[reg] = xmm_pattern(task, reg);
        if avx {
            ymm_high[reg] = ymm_high_pattern(task, reg);
        }
        reg += 1;
    }
    SimdFile {
        xmm,
        ymm_high,
        mxcsr: mxcsr_pattern(task),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimdReport {
    pub xmm_mask: u16,
    pub ymm_mask: u16,
    pub mxcsr_mismatch: bool,
}

impl SimdReport {
    pub const fn is_clean(&self) -> bool {
        self.xmm_mask == 0 && self.ymm_mask == 0 && !self.mxcsr_mismatch
    }

    pub fn first_corrupted_xmm(&self) -> Option<usize> {
        (0..SIMD_REGISTERS).find(|reg| self.xmm_mask & (1 << reg) != 0)
    }

    pub fn first_corrupted_ymm(&self) -> Option<usize> {
        (0..SIMD_REGISTERS).find(|reg| self.ymm_mask & (1 << reg) != 0)
    }
}

/// Compare XMM, MXCSR et, si AVX est actif, les moitiés hautes YMM.
pub fn compare_simd(expected: &SimdFile, observed: &SimdFile, avx: bool) -> SimdReport {
    let mut xmm_mask = 0u16;
    let mut ymm_mask = 0u16;
    for reg in 0..SIMD_REGISTERS {
        if expected.xmm[reg] != observed.xmm[reg] {
            xmm_mask |= 1 << reg;
        }
        if avx && expected.ymm_high[reg] != observed.ymm_high[reg] {
            ymm_mask |= 1 << reg;
        }
    }
    SimdReport {
        xmm_mask,
        ymm_mask,
        mxcsr_mismatch: expected.mxcsr != observed.mxcsr,
    }
}
