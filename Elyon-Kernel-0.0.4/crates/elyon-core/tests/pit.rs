//! Tests hôte de l'attente PIT canal 2 et de la calibration du timer LAPIC.

use std::collections::VecDeque;

use elyon_core::pit::{self, CalibrationError, PIT_FREQUENCY_HZ, PitError, PitPort};

/// PIT simulé : rejoue une séquence d'états OUT2 et de valeurs latchées.
struct FakePit {
    out2: VecDeque<bool>,
    latched: VecDeque<u16>,
    polls: usize,
}

impl FakePit {
    fn new(out2: &[bool], latched: &[u16]) -> Self {
        Self {
            out2: out2.iter().copied().collect(),
            latched: latched.iter().copied().collect(),
            polls: 0,
        }
    }
}

impl PitPort for FakePit {
    fn out2_high(&mut self) -> bool {
        self.polls += 1;
        self.out2.pop_front().unwrap_or(false)
    }

    fn latched_count(&mut self) -> u16 {
        self.latched.pop_front().unwrap_or(0)
    }
}

// --- Attente du compte terminal (H1) ----------------------------------------------

#[test]
fn out2_already_high_on_first_poll_completes_immediately() {
    // Reproduit H1 : sous hyperviseur, le premier `in 0x61` peut arriver après
    // le compte terminal. L'ancienne boucle attendait un front bas puis haut
    // et tournait jusqu'à PIT_MAX_POLLS avant d'échouer.
    let mut fake = FakePit::new(&[true], &[]);
    assert_eq!(pit::wait_for_terminal_count(&mut fake, 1_000), Ok(1));
    assert_eq!(fake.polls, 1);
}

#[test]
fn out2_rising_after_some_polls_completes() {
    let mut fake = FakePit::new(&[false, false, false, true], &[]);
    assert_eq!(pit::wait_for_terminal_count(&mut fake, 1_000), Ok(4));
}

#[test]
fn out2_never_rising_times_out_after_budget() {
    let mut fake = FakePit::new(&[], &[]);
    assert_eq!(
        pit::wait_for_terminal_count(&mut fake, 500),
        Err(PitError::Timeout)
    );
    assert_eq!(fake.polls, 500);
}

#[test]
fn zero_poll_budget_times_out_without_polling() {
    let mut fake = FakePit::new(&[true], &[]);
    assert_eq!(
        pit::wait_for_terminal_count(&mut fake, 0),
        Err(PitError::Timeout)
    );
    assert_eq!(fake.polls, 0);
}

// --- Mesure exacte via le compteur latché ------------------------------------------

#[test]
fn latched_zero_means_exactly_terminal_count() {
    assert_eq!(pit::elapsed_pit_ticks(11_932, 0), 11_932);
}

#[test]
fn latched_wrap_adds_overshoot_after_terminal_count() {
    // En mode 0 le compteur passe de 0 à 0xFFFF puis continue à décompter.
    assert_eq!(pit::elapsed_pit_ticks(11_932, 0xFFFF), 11_933);
    assert_eq!(pit::elapsed_pit_ticks(11_932, 0xFF00), 11_932 + 0x100);
    assert_eq!(pit::elapsed_pit_ticks(238, 0x0001), 238 + 0xFFFF);
}

#[test]
fn latched_count_is_read_through_the_port() {
    let mut fake = FakePit::new(&[true], &[0xFFF0]);
    pit::wait_for_terminal_count(&mut fake, 10).unwrap();
    let latched = fake.latched_count();
    assert_eq!(pit::elapsed_pit_ticks(1_000, latched), 1_016);
}

// --- Conversion délai -> compte PIT -------------------------------------------------

#[test]
fn delay_is_converted_to_pit_count() {
    assert_eq!(pit::pit_count_for_us(10_000), 11_931);
    assert_eq!(pit::pit_count_for_us(1_000), 1_193);
    assert_eq!(pit::pit_count_for_us(200), 238);
}

#[test]
fn delay_count_is_clamped() {
    assert_eq!(pit::pit_count_for_us(0), 1);
    assert_eq!(pit::pit_count_for_us(1), 1);
    assert_eq!(pit::pit_count_for_us(1_000_000), u16::MAX);
}

// --- Calibration LAPIC ---------------------------------------------------------------

#[test]
fn qemu_measurement_gives_expected_frequency() {
    // Valeurs du journal CI : lapic_elapsed=629575 sur 11932 ticks PIT.
    let hz = pit::lapic_frequency(629_575, 11_932).unwrap();
    assert_eq!(hz, 629_575 * PIT_FREQUENCY_HZ / 11_932);
    assert_eq!(pit::periodic_initial_count(hz, 100), Ok(629_565));
}

#[test]
fn overshoot_is_compensated_by_measured_pit_ticks() {
    // Même horloge LAPIC, fenêtre PIT observée 2x plus longue : même fréquence.
    let nominal = pit::lapic_frequency(600_000, 11_932).unwrap();
    let late = pit::lapic_frequency(1_200_000, 23_864).unwrap();
    assert_eq!(nominal, late);
}

#[test]
fn lapic_that_did_not_advance_is_rejected() {
    assert_eq!(
        pit::lapic_frequency(0, 11_932),
        Err(CalibrationError::LapicDidNotAdvance)
    );
}

#[test]
fn empty_pit_window_is_rejected() {
    assert_eq!(
        pit::lapic_frequency(600_000, 0),
        Err(CalibrationError::NoPitTicks)
    );
}

#[test]
fn implausibly_slow_lapic_is_rejected() {
    // 100 ticks LAPIC en 10 ms : 10 kHz, sous la borne basse.
    assert_eq!(
        pit::lapic_frequency(100, 11_932),
        Err(CalibrationError::ImplausibleFrequency)
    );
}

#[test]
fn implausibly_fast_lapic_is_rejected() {
    // u32::MAX ticks LAPIC en 1 tick PIT : plusieurs PHz.
    assert_eq!(
        pit::lapic_frequency(u32::MAX, 1),
        Err(CalibrationError::ImplausibleFrequency)
    );
}

#[test]
fn frequency_bounds_are_inclusive() {
    assert!(pit::lapic_frequency_is_plausible(pit::MIN_LAPIC_HZ));
    assert!(pit::lapic_frequency_is_plausible(pit::MAX_LAPIC_HZ));
    assert!(!pit::lapic_frequency_is_plausible(pit::MIN_LAPIC_HZ - 1));
    assert!(!pit::lapic_frequency_is_plausible(pit::MAX_LAPIC_HZ + 1));
}

#[test]
fn zero_target_or_initial_count_is_rejected() {
    assert_eq!(
        pit::periodic_initial_count(1_000_000, 0),
        Err(CalibrationError::InvalidInitialCount)
    );
    assert_eq!(
        pit::periodic_initial_count(50, 100),
        Err(CalibrationError::InvalidInitialCount)
    );
}

#[test]
fn initial_count_above_u32_is_rejected() {
    assert_eq!(
        pit::periodic_initial_count(u64::MAX, 1),
        Err(CalibrationError::InvalidInitialCount)
    );
}

// --- Repli CPUID 0x40000010 (VMware) --------------------------------------------------

#[test]
fn vmware_timing_leaf_gives_divided_lapic_frequency() {
    // EBX = fréquence du bus LAPIC en kHz (66 MHz sous VMware).
    assert_eq!(
        pit::hypervisor_lapic_hz(0x4000_0010, 66_000, 16),
        Some(66_000_000 / 16)
    );
}

#[test]
fn timing_leaf_absent_gives_no_frequency() {
    assert_eq!(pit::hypervisor_lapic_hz(0x4000_0001, 66_000, 16), None);
    assert_eq!(pit::hypervisor_lapic_hz(0, 66_000, 16), None);
}

#[test]
fn timing_leaf_with_zero_bus_frequency_gives_no_frequency() {
    assert_eq!(pit::hypervisor_lapic_hz(0x4000_0010, 0, 16), None);
}

#[test]
fn timing_leaf_frequency_must_be_plausible() {
    assert_eq!(pit::hypervisor_lapic_hz(0x4000_0010, 1, 16), None);
    assert_eq!(pit::hypervisor_lapic_hz(0x4000_0010, 66_000, 0), None);
}
