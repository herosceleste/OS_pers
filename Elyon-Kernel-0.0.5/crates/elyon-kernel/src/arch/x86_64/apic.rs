use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::acpi::AcpiTopology;
use crate::arch::x86_64::{cpu, paging};

const IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_BSP: u64 = 1 << 8;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_MASK: u64 = 0x0000_000F_FFFF_F000;

const XAPIC_ID: u64 = 0x020;
const XAPIC_VERSION: u64 = 0x030;
const XAPIC_TPR: u64 = 0x080;
const XAPIC_EOI: u64 = 0x0B0;
const XAPIC_SVR: u64 = 0x0F0;
const XAPIC_ICR_LOW: u64 = 0x300;
const XAPIC_ICR_HIGH: u64 = 0x310;
const XAPIC_LVT_TIMER: u64 = 0x320;
const XAPIC_TIMER_INITIAL: u64 = 0x380;
const XAPIC_TIMER_CURRENT: u64 = 0x390;
const XAPIC_TIMER_DIVIDE: u64 = 0x3E0;

const X2APIC_ID: u32 = 0x802;
const X2APIC_VERSION: u32 = 0x803;
const X2APIC_TPR: u32 = 0x808;
const X2APIC_EOI: u32 = 0x80B;
const X2APIC_SVR: u32 = 0x80F;
const X2APIC_ICR: u32 = 0x830;
const X2APIC_LVT_TIMER: u32 = 0x832;
const X2APIC_TIMER_INITIAL: u32 = 0x838;
const X2APIC_TIMER_CURRENT: u32 = 0x839;
const X2APIC_TIMER_DIVIDE: u32 = 0x83E;

const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;

const ICR_DELIVERY_STATUS: u32 = 1 << 12;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_TRIGGER_LEVEL: u32 = 1 << 15;
const ICR_DELIVERY_INIT: u32 = 0b101 << 8;
const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
const ICR_WAIT_SPINS: usize = 10_000_000;

pub const SPURIOUS_VECTOR: u8 = 0xFF;

const MODE_NONE: u8 = 0;
const MODE_XAPIC: u8 = 1;
const MODE_X2APIC: u8 = 2;

static ACTIVE_MODE: AtomicU8 = AtomicU8::new(MODE_NONE);
static ACTIVE_BASE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocalApicMode {
    XApic,
    X2Apic,
}

impl LocalApicMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XApic => "xapic",
            Self::X2Apic => "x2apic",
        }
    }
}

pub struct LocalApicState {
    pub mode: LocalApicMode,
    pub apic_id: u32,
    pub version: u32,
    pub msr_base: u64,
    pub madt_base: u64,
    pub is_bsp: bool,
    pub cpuid_apic: bool,
    pub cpuid_x2apic: bool,
    pub svr: u32,
}

impl LocalApicState {
    pub const fn software_enabled(&self) -> bool {
        self.svr & (1 << 8) != 0
    }
}

#[derive(Clone, Copy)]
pub enum ApicError {
    ApicUnsupported,
    X2ApicStateUnsupported,
    InvalidMadtBase,
    BaseMismatch,
    MmioOutsideIdentityMap,
    BspNotInMadt,
    ControllerNotInitialized,
    IcrBusyTimeout,
}

impl ApicError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApicUnsupported => "CPUID reports no Local APIC",
            Self::X2ApicStateUnsupported => "x2APIC enabled but CPUID does not advertise x2APIC",
            Self::InvalidMadtBase => "MADT Local APIC address is zero",
            Self::BaseMismatch => "IA32_APIC_BASE does not match MADT Local APIC address",
            Self::MmioOutsideIdentityMap => "Local APIC MMIO outside current identity map",
            Self::BspNotInMadt => "current BSP APIC ID absent from usable MADT CPUs",
            Self::ControllerNotInitialized => "Local APIC controller not initialized",
            Self::IcrBusyTimeout => "Local APIC ICR delivery-status timeout",
        }
    }
}

pub fn init(topology: &AcpiTopology) -> Result<LocalApicState, ApicError> {
    let caps = cpu::apic_capabilities();
    if !caps.apic {
        return Err(ApicError::ApicUnsupported);
    }
    if topology.local_apic_address == 0 {
        return Err(ApicError::InvalidMadtBase);
    }

    let mut apic_base = unsafe { cpu::read_msr(IA32_APIC_BASE) };
    let is_bsp = apic_base & APIC_BASE_BSP != 0;

    if apic_base & APIC_BASE_ENABLE == 0 {
        apic_base |= APIC_BASE_ENABLE;
        unsafe { cpu::write_msr(IA32_APIC_BASE, apic_base) };
        apic_base = unsafe { cpu::read_msr(IA32_APIC_BASE) };
    }

    let mode = if apic_base & APIC_BASE_X2APIC != 0 {
        if !caps.x2apic {
            return Err(ApicError::X2ApicStateUnsupported);
        }
        LocalApicMode::X2Apic
    } else {
        LocalApicMode::XApic
    };

    let msr_base = apic_base & APIC_BASE_MASK;

    let (apic_id, version, svr) = match mode {
        LocalApicMode::XApic => {
            if msr_base != topology.local_apic_address {
                return Err(ApicError::BaseMismatch);
            }
            if !paging::mark_identity_mmio_uncacheable(msr_base) {
                return Err(ApicError::MmioOutsideIdentityMap);
            }

            write_xapic(msr_base, XAPIC_TPR, 0);
            let current_svr = read_xapic(msr_base, XAPIC_SVR);
            let svr = (current_svr & !0xFF) | SPURIOUS_VECTOR as u32 | (1 << 8);
            write_xapic(msr_base, XAPIC_SVR, svr);

            (
                read_xapic(msr_base, XAPIC_ID) >> 24,
                read_xapic(msr_base, XAPIC_VERSION),
                read_xapic(msr_base, XAPIC_SVR),
            )
        }
        LocalApicMode::X2Apic => {
            unsafe { cpu::write_msr(X2APIC_TPR, 0) };
            let current_svr = unsafe { cpu::read_msr(X2APIC_SVR) } as u32;
            let svr = (current_svr & !0xFF) | SPURIOUS_VECTOR as u32 | (1 << 8);
            unsafe { cpu::write_msr(X2APIC_SVR, svr as u64) };

            (
                unsafe { cpu::read_msr(X2APIC_ID) } as u32,
                unsafe { cpu::read_msr(X2APIC_VERSION) } as u32,
                unsafe { cpu::read_msr(X2APIC_SVR) } as u32,
            )
        }
    };

    if !topology
        .cpus()
        .iter()
        .any(|candidate| candidate.usable() && candidate.apic_id == apic_id)
    {
        return Err(ApicError::BspNotInMadt);
    }

    ACTIVE_BASE.store(msr_base, Ordering::Release);
    ACTIVE_MODE.store(
        match mode {
            LocalApicMode::XApic => MODE_XAPIC,
            LocalApicMode::X2Apic => MODE_X2APIC,
        },
        Ordering::Release,
    );

    Ok(LocalApicState {
        mode,
        apic_id,
        version,
        msr_base,
        madt_base: topology.local_apic_address,
        is_bsp,
        cpuid_apic: caps.apic,
        cpuid_x2apic: caps.x2apic,
        svr,
    })
}

pub fn eoi() -> bool {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return false;
            }
            write_xapic(base, XAPIC_EOI, 0);
            true
        }
        MODE_X2APIC => {
            unsafe { cpu::write_msr(X2APIC_EOI, 0) };
            true
        }
        _ => false,
    }
}

fn read_xapic(base: u64, offset: u64) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write_xapic(base: u64, offset: u64, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

pub fn timer_begin_calibration(divide_config: u32) -> bool {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return false;
            }
            write_xapic(base, XAPIC_TIMER_DIVIDE, divide_config);
            write_xapic(base, XAPIC_LVT_TIMER, LVT_MASKED);
            write_xapic(base, XAPIC_TIMER_INITIAL, u32::MAX);
            true
        }
        MODE_X2APIC => {
            unsafe {
                cpu::write_msr(X2APIC_TIMER_DIVIDE, divide_config as u64);
                cpu::write_msr(X2APIC_LVT_TIMER, LVT_MASKED as u64);
                cpu::write_msr(X2APIC_TIMER_INITIAL, u32::MAX as u64);
            }
            true
        }
        _ => false,
    }
}

pub fn timer_current_count() -> Option<u32> {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                None
            } else {
                Some(read_xapic(base, XAPIC_TIMER_CURRENT))
            }
        }
        MODE_X2APIC => Some(unsafe { cpu::read_msr(X2APIC_TIMER_CURRENT) } as u32),
        _ => None,
    }
}

pub fn timer_start_periodic(vector: u8, initial_count: u32, divide_config: u32) -> bool {
    if initial_count == 0 {
        return false;
    }

    let lvt = vector as u32 | LVT_TIMER_PERIODIC;

    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return false;
            }
            write_xapic(base, XAPIC_TIMER_DIVIDE, divide_config);
            write_xapic(base, XAPIC_LVT_TIMER, lvt);
            write_xapic(base, XAPIC_TIMER_INITIAL, initial_count);
            true
        }
        MODE_X2APIC => {
            unsafe {
                cpu::write_msr(X2APIC_TIMER_DIVIDE, divide_config as u64);
                cpu::write_msr(X2APIC_LVT_TIMER, lvt as u64);
                cpu::write_msr(X2APIC_TIMER_INITIAL, initial_count as u64);
            }
            true
        }
        _ => false,
    }
}

pub fn timer_stop() -> bool {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return false;
            }
            let lvt = read_xapic(base, XAPIC_LVT_TIMER);
            write_xapic(base, XAPIC_LVT_TIMER, lvt | LVT_MASKED);
            write_xapic(base, XAPIC_TIMER_INITIAL, 0);
            true
        }
        MODE_X2APIC => {
            let lvt = unsafe { cpu::read_msr(X2APIC_LVT_TIMER) } as u32;
            unsafe {
                cpu::write_msr(X2APIC_LVT_TIMER, (lvt | LVT_MASKED) as u64);
                cpu::write_msr(X2APIC_TIMER_INITIAL, 0);
            }
            true
        }
        _ => false,
    }
}

pub fn init_secondary() -> Result<u32, ApicError> {
    let caps = cpu::apic_capabilities();
    if !caps.apic {
        return Err(ApicError::ApicUnsupported);
    }

    let mode = ACTIVE_MODE.load(Ordering::Acquire);
    let base = ACTIVE_BASE.load(Ordering::Acquire);
    if mode == MODE_NONE || base == 0 {
        return Err(ApicError::ControllerNotInitialized);
    }

    let mut apic_base = unsafe { cpu::read_msr(IA32_APIC_BASE) };
    if apic_base & APIC_BASE_ENABLE == 0 {
        apic_base |= APIC_BASE_ENABLE;
        unsafe { cpu::write_msr(IA32_APIC_BASE, apic_base) };
    }

    match mode {
        MODE_XAPIC => {
            write_xapic(base, XAPIC_TPR, 0);
            let current_svr = read_xapic(base, XAPIC_SVR);
            let svr = (current_svr & !0xFF) | SPURIOUS_VECTOR as u32 | (1 << 8);
            write_xapic(base, XAPIC_SVR, svr);
            Ok(read_xapic(base, XAPIC_ID) >> 24)
        }
        MODE_X2APIC => unsafe {
            cpu::write_msr(X2APIC_TPR, 0);
            let current_svr = cpu::read_msr(X2APIC_SVR) as u32;
            let svr = (current_svr & !0xFF) | SPURIOUS_VECTOR as u32 | (1 << 8);
            cpu::write_msr(X2APIC_SVR, svr as u64);
            Ok(cpu::read_msr(X2APIC_ID) as u32)
        },
        _ => Err(ApicError::ControllerNotInitialized),
    }
}

const ICR_DELIVERY_NMI: u32 = 0b100 << 8;
const ICR_SHORTHAND_ALL_BUT_SELF: u32 = 0b11 << 18;

/// NMI à tous les CPU sauf l'émetteur (arrêt système, K004-10). Le champ
/// destination est ignoré avec ce raccourci.
pub fn send_nmi_to_others() -> Result<(), ApicError> {
    send_icr(
        0,
        ICR_DELIVERY_NMI | ICR_LEVEL_ASSERT | ICR_SHORTHAND_ALL_BUT_SELF,
    )
}

/// NMI vers un seul CPU (qualification de l'entrée paranoïaque, K005-03).
pub fn send_nmi(apic_id: u32) -> Result<(), ApicError> {
    send_icr(apic_id, ICR_DELIVERY_NMI | ICR_LEVEL_ASSERT)
}

pub fn send_init(apic_id: u32) -> Result<(), ApicError> {
    send_icr(
        apic_id,
        ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL,
    )
}

pub fn send_startup(apic_id: u32, vector: u8) -> Result<(), ApicError> {
    send_icr(apic_id, ICR_DELIVERY_STARTUP | vector as u32)
}

fn send_icr(apic_id: u32, low: u32) -> Result<(), ApicError> {
    wait_icr_idle()?;

    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            if apic_id > u8::MAX as u32 {
                return Err(ApicError::ControllerNotInitialized);
            }
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return Err(ApicError::ControllerNotInitialized);
            }
            write_xapic(base, XAPIC_ICR_HIGH, apic_id << 24);
            write_xapic(base, XAPIC_ICR_LOW, low);
        }
        MODE_X2APIC => unsafe {
            cpu::write_msr(X2APIC_ICR, ((apic_id as u64) << 32) | low as u64);
        },
        _ => return Err(ApicError::ControllerNotInitialized),
    }

    wait_icr_idle()
}

fn wait_icr_idle() -> Result<(), ApicError> {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return Err(ApicError::ControllerNotInitialized);
            }

            for _ in 0..ICR_WAIT_SPINS {
                if read_xapic(base, XAPIC_ICR_LOW) & ICR_DELIVERY_STATUS == 0 {
                    return Ok(());
                }
                core::hint::spin_loop();
            }
            Err(ApicError::IcrBusyTimeout)
        }
        MODE_X2APIC => Ok(()),
        _ => Err(ApicError::ControllerNotInitialized),
    }
}

pub fn current_id() -> Result<u32, ApicError> {
    match ACTIVE_MODE.load(Ordering::Acquire) {
        MODE_XAPIC => {
            let base = ACTIVE_BASE.load(Ordering::Acquire);
            if base == 0 {
                return Err(ApicError::ControllerNotInitialized);
            }
            Ok(read_xapic(base, XAPIC_ID) >> 24)
        }
        MODE_X2APIC => Ok(unsafe { cpu::read_msr(X2APIC_ID) } as u32),
        _ => Err(ApicError::ControllerNotInitialized),
    }
}

pub fn send_fixed(apic_id: u32, vector: u8) -> Result<(), ApicError> {
    if vector < 0x20 {
        return Err(ApicError::ControllerNotInitialized);
    }
    send_icr(apic_id, vector as u32)
}
