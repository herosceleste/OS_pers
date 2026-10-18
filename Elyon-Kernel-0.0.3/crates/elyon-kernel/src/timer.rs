use core::hint::spin_loop;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::{apic, cpu};

const PIT_CHANNEL2_DATA: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const PIT_SPEAKER_CONTROL: u16 = 0x61;

const PIC_MASTER_DATA: u16 = 0x21;
const PIC_SLAVE_DATA: u16 = 0xA1;

const PIT_FREQUENCY_HZ: u64 = 1_193_182;
const PIT_CALIBRATION_COUNT: u16 = 11_932;
const PIT_MAX_POLLS: usize = 50_000_000;

const APIC_DIVIDE_BY_16_CONFIG: u32 = 0x3;

pub const TIMER_VECTOR: u8 = 0xE0;
pub const TIMER_HZ: u32 = 100;
pub const QUALIFICATION_TICKS: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);
static EOI_FAILURES: AtomicU64 = AtomicU64::new(0);

pub struct TimerCalibration {
    pub pit_count: u16,
    pub lapic_elapsed: u32,
    pub effective_lapic_hz: u64,
    pub initial_count: u32,
    pub target_hz: u32,
    pub divide: u32,
}

#[derive(Clone, Copy)]
pub enum TimerError {
    LapicUnavailable,
    PitTimeout,
    LapicDidNotAdvance,
    InvalidInitialCount,
    PeriodicProgrammingFailed,
    StopFailed,
    EoiFailure,
}

impl TimerError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LapicUnavailable => "Local APIC timer unavailable",
            Self::PitTimeout => "PIT channel 2 calibration timeout",
            Self::LapicDidNotAdvance => "Local APIC timer did not advance",
            Self::InvalidInitialCount => "invalid Local APIC periodic initial count",
            Self::PeriodicProgrammingFailed => "failed to program Local APIC periodic timer",
            Self::StopFailed => "failed to stop Local APIC timer",
            Self::EoiFailure => "Local APIC EOI failed during timer interrupt",
        }
    }
}

pub fn calibrate_and_start() -> Result<TimerCalibration, TimerError> {
    mask_legacy_pic();
    TICKS.store(0, Ordering::Release);
    EOI_FAILURES.store(0, Ordering::Release);

    if !apic::timer_begin_calibration(APIC_DIVIDE_BY_16_CONFIG) {
        return Err(TimerError::LapicUnavailable);
    }

    start_pit_channel2(PIT_CALIBRATION_COUNT);
    wait_for_pit_channel2()?;

    let current = apic::timer_current_count().ok_or(TimerError::LapicUnavailable)?;
    stop_pit_channel2();

    let elapsed = u32::MAX.wrapping_sub(current);
    if elapsed == 0 {
        return Err(TimerError::LapicDidNotAdvance);
    }

    let effective_lapic_hz = elapsed as u64 * PIT_FREQUENCY_HZ / PIT_CALIBRATION_COUNT as u64;
    let initial_count = effective_lapic_hz / TIMER_HZ as u64;

    if initial_count == 0 || initial_count > u32::MAX as u64 {
        return Err(TimerError::InvalidInitialCount);
    }

    if !apic::timer_start_periodic(TIMER_VECTOR, initial_count as u32, APIC_DIVIDE_BY_16_CONFIG) {
        return Err(TimerError::PeriodicProgrammingFailed);
    }

    Ok(TimerCalibration {
        pit_count: PIT_CALIBRATION_COUNT,
        lapic_elapsed: elapsed,
        effective_lapic_hz,
        initial_count: initial_count as u32,
        target_hz: TIMER_HZ,
        divide: 16,
    })
}

pub fn on_interrupt() {
    TICKS.fetch_add(1, Ordering::AcqRel);
    if !apic::eoi() {
        EOI_FAILURES.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

pub fn eoi_failures() -> u64 {
    EOI_FAILURES.load(Ordering::Acquire)
}

pub fn wait_for_ticks(target: u64) -> u64 {
    loop {
        let current = ticks();
        if current >= target {
            return current;
        }
        cpu::halt_once();
    }
}

pub fn stop() -> Result<(), TimerError> {
    if !apic::timer_stop() {
        return Err(TimerError::StopFailed);
    }
    if eoi_failures() != 0 {
        return Err(TimerError::EoiFailure);
    }
    Ok(())
}

pub fn mask_legacy_pic() {
    unsafe {
        cpu::out_u8(PIC_MASTER_DATA, 0xFF);
        cpu::out_u8(PIC_SLAVE_DATA, 0xFF);
    }
}

fn start_pit_channel2(count: u16) {
    unsafe {
        let control = cpu::in_u8(PIT_SPEAKER_CONTROL);
        cpu::out_u8(PIT_SPEAKER_CONTROL, control & !0x03);

        cpu::out_u8(PIT_COMMAND, 0xB0);
        cpu::out_u8(PIT_CHANNEL2_DATA, count as u8);
        cpu::out_u8(PIT_CHANNEL2_DATA, (count >> 8) as u8);

        cpu::out_u8(PIT_SPEAKER_CONTROL, (control & !0x02) | 0x01);
    }
}

fn wait_for_pit_channel2() -> Result<(), TimerError> {
    let mut saw_low = false;

    for _ in 0..PIT_MAX_POLLS {
        let status = unsafe { cpu::in_u8(PIT_SPEAKER_CONTROL) };

        if !saw_low {
            if status & 0x20 == 0 {
                saw_low = true;
            }
        } else if status & 0x20 != 0 {
            return Ok(());
        }

        spin_loop();
    }

    Err(TimerError::PitTimeout)
}

fn stop_pit_channel2() {
    unsafe {
        let control = cpu::in_u8(PIT_SPEAKER_CONTROL);
        cpu::out_u8(PIT_SPEAKER_CONTROL, control & !0x03);
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_timer_interrupt_dispatch() {
    on_interrupt();
}
