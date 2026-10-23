use core::sync::atomic::{AtomicU64, Ordering};

use elyon_core::pit::{self, CalibrationError, PitPort};

use crate::arch::x86_64::{apic, cpu, percpu};

const PIT_CHANNEL2_DATA: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const PIT_SPEAKER_CONTROL: u16 = 0x61;

/// Canal 2, accès octet bas puis haut, mode 0, binaire.
const PIT_CHANNEL2_MODE0: u8 = 0xB0;
/// Canal 2, commande Counter Latch.
const PIT_CHANNEL2_LATCH: u8 = 0x80;
const PIT_OUT2: u8 = 0x20;

const PIC_MASTER_DATA: u16 = 0x21;
const PIC_SLAVE_DATA: u16 = 0xA1;

const PIT_CALIBRATION_COUNT: u16 = 11_932;
const PIT_MAX_POLLS: usize = 50_000_000;

const APIC_DIVIDE_BY_16_CONFIG: u32 = 0x3;
const APIC_DIVIDE: u32 = 16;

/// Marge accordée à `wait_for_ticks` par rapport à la durée nominale.
const TICK_WAIT_MARGIN: u64 = 10;
const TICK_WAIT_STEP_US: u32 = 1_000;

pub const TIMER_VECTOR: u8 = 0xE0;
pub const TIMER_HZ: u32 = 100;
pub const QUALIFICATION_TICKS: u64 = 100;

static TICKS: AtomicU64 = AtomicU64::new(0);
static EOI_FAILURES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub enum CalibrationSource {
    Pit2,
    HypervisorLeaf,
}

impl CalibrationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pit2 => "PIT2",
            Self::HypervisorLeaf => "CPUID40000010",
        }
    }
}

pub struct TimerCalibration {
    pub source: CalibrationSource,
    /// Ticks PIT réellement écoulés (compteur latché), 0 si source hyperviseur.
    pub pit_ticks: u32,
    pub lapic_elapsed: u32,
    pub effective_lapic_hz: u64,
    pub initial_count: u32,
    pub target_hz: u32,
    pub divide: u32,
    /// Raison du repli quand `source` vaut `HypervisorLeaf`.
    pub pit_failure: Option<TimerError>,
}

#[derive(Clone, Copy)]
pub enum TimerError {
    LapicUnavailable,
    PitTimeout,
    Calibration(CalibrationError),
    PeriodicProgrammingFailed,
    StopFailed,
    EoiFailure,
    TickTimeout,
}

impl TimerError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LapicUnavailable => "Local APIC timer unavailable",
            Self::PitTimeout => "PIT channel 2 calibration timeout",
            Self::Calibration(error) => error.as_str(),
            Self::PeriodicProgrammingFailed => "failed to program Local APIC periodic timer",
            Self::StopFailed => "failed to stop Local APIC timer",
            Self::EoiFailure => "Local APIC EOI failed during timer interrupt",
            Self::TickTimeout => "timer interrupts did not reach the target in time",
        }
    }
}

/// PIT canal 2 via les ports legacy.
struct LegacyPit;

impl PitPort for LegacyPit {
    fn out2_high(&mut self) -> bool {
        // SAFETY: lecture du port 0x61 (contrôle speaker/PIT2), sans effet de bord.
        unsafe { cpu::in_u8(PIT_SPEAKER_CONTROL) & PIT_OUT2 != 0 }
    }

    fn latched_count(&mut self) -> u16 {
        // SAFETY: la commande Counter Latch fige le compteur du canal 2, programmé
        // en accès octet bas/haut ; les deux lectures de 0x42 le restituent.
        unsafe {
            cpu::out_u8(PIT_COMMAND, PIT_CHANNEL2_LATCH);
            let low = cpu::in_u8(PIT_CHANNEL2_DATA);
            let high = cpu::in_u8(PIT_CHANNEL2_DATA);
            u16::from_le_bytes([low, high])
        }
    }
}

pub fn calibrate_and_start() -> Result<TimerCalibration, TimerError> {
    mask_legacy_pic();
    TICKS.store(0, Ordering::Release);
    EOI_FAILURES.store(0, Ordering::Release);
    percpu::reset_bsp_ticks();

    if !apic::timer_begin_calibration(APIC_DIVIDE_BY_16_CONFIG) {
        return Err(TimerError::LapicUnavailable);
    }

    let mut calibration = match measure_with_pit() {
        Ok(calibration) => calibration,
        Err(pit_failure) => {
            let (max_leaf, timing_ebx) = cpu::hypervisor_timing_leaf();
            let hz =
                pit::hypervisor_lapic_hz(max_leaf, timing_ebx, APIC_DIVIDE).ok_or(pit_failure)?;
            TimerCalibration {
                source: CalibrationSource::HypervisorLeaf,
                pit_ticks: 0,
                lapic_elapsed: 0,
                effective_lapic_hz: hz,
                initial_count: 0,
                target_hz: TIMER_HZ,
                divide: APIC_DIVIDE,
                pit_failure: Some(pit_failure),
            }
        }
    };

    calibration.initial_count =
        pit::periodic_initial_count(calibration.effective_lapic_hz, TIMER_HZ)
            .map_err(TimerError::Calibration)?;

    if !apic::timer_start_periodic(
        TIMER_VECTOR,
        calibration.initial_count,
        APIC_DIVIDE_BY_16_CONFIG,
    ) {
        return Err(TimerError::PeriodicProgrammingFailed);
    }

    Ok(calibration)
}

/// Mesure la fréquence LAPIC sur une fenêtre PIT d'environ 10 ms. Le compteur
/// PIT est latché juste après la lecture LAPIC : le dépassement éventuel
/// (sortie VM tardive) est compté des deux côtés et n'affecte pas le ratio.
fn measure_with_pit() -> Result<TimerCalibration, TimerError> {
    let mut port = LegacyPit;

    start_pit_channel2(PIT_CALIBRATION_COUNT);
    let waited = pit::wait_for_terminal_count(&mut port, PIT_MAX_POLLS);
    let current = apic::timer_current_count();
    let latched = port.latched_count();
    stop_pit_channel2();

    waited.map_err(|_| TimerError::PitTimeout)?;
    let current = current.ok_or(TimerError::LapicUnavailable)?;

    let lapic_elapsed = u32::MAX.wrapping_sub(current);
    let pit_ticks = pit::elapsed_pit_ticks(PIT_CALIBRATION_COUNT, latched);
    let effective_lapic_hz =
        pit::lapic_frequency(lapic_elapsed, pit_ticks).map_err(TimerError::Calibration)?;

    Ok(TimerCalibration {
        source: CalibrationSource::Pit2,
        pit_ticks,
        lapic_elapsed,
        effective_lapic_hz,
        initial_count: 0,
        target_hz: TIMER_HZ,
        divide: APIC_DIVIDE,
        pit_failure: None,
    })
}

pub fn on_interrupt() {
    TICKS.fetch_add(1, Ordering::AcqRel);
    percpu::increment_bsp_ticks();
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

/// Attend `target` ticks, IF=1 requis. Échoue au-delà de `TICK_WAIT_MARGIN`
/// fois la durée nominale au lieu de bloquer en `hlt` si le timer se tait.
pub fn wait_for_ticks(target: u64) -> Result<u64, TimerError> {
    let nominal_ms = target.saturating_mul(1_000) / TIMER_HZ as u64;
    let budget_steps = nominal_ms.saturating_mul(TICK_WAIT_MARGIN).max(1);

    for _ in 0..budget_steps {
        let current = ticks();
        if current >= target {
            return Ok(current);
        }
        pit_delay_us(TICK_WAIT_STEP_US)?;
    }

    let current = ticks();
    if current >= target {
        Ok(current)
    } else {
        Err(TimerError::TickTimeout)
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
    // SAFETY: écrire 0xFF dans les registres IMR des deux 8259 masque toutes
    // les IRQ legacy ; aucun autre état n'est modifié.
    unsafe {
        cpu::out_u8(PIC_MASTER_DATA, 0xFF);
        cpu::out_u8(PIC_SLAVE_DATA, 0xFF);
    }
}

fn start_pit_channel2(count: u16) {
    // SAFETY: séquence 8254 standard sur le canal 2 : gate coupé, mode 0 et
    // compte chargés, puis gate rétabli (haut-parleur toujours coupé).
    unsafe {
        let control = cpu::in_u8(PIT_SPEAKER_CONTROL);
        cpu::out_u8(PIT_SPEAKER_CONTROL, control & !0x03);

        cpu::out_u8(PIT_COMMAND, PIT_CHANNEL2_MODE0);
        cpu::out_u8(PIT_CHANNEL2_DATA, count as u8);
        cpu::out_u8(PIT_CHANNEL2_DATA, (count >> 8) as u8);

        cpu::out_u8(PIT_SPEAKER_CONTROL, (control & !0x02) | 0x01);
    }
}

fn stop_pit_channel2() {
    // SAFETY: coupe le gate du canal 2 et le haut-parleur.
    unsafe {
        let control = cpu::in_u8(PIT_SPEAKER_CONTROL);
        cpu::out_u8(PIT_SPEAKER_CONTROL, control & !0x03);
    }
}

pub fn pit_delay_us(microseconds: u32) -> Result<(), TimerError> {
    if microseconds == 0 {
        return Ok(());
    }

    start_pit_channel2(pit::pit_count_for_us(microseconds));
    let result = pit::wait_for_terminal_count(&mut LegacyPit, PIT_MAX_POLLS);
    stop_pit_channel2();
    result.map(|_| ()).map_err(|_| TimerError::PitTimeout)
}

#[unsafe(no_mangle)]
extern "C" fn elyon_timer_interrupt_dispatch() {
    on_interrupt();
}
