//! Attente PIT canal 2 (mode 0) et calibration du timer Local APIC, sans E/S.
//!
//! Le noyau implémente [`PitPort`] sur les ports 0x61/0x42/0x43 ; ce module
//! décide quand le compte terminal est atteint et ce que valent les mesures.

pub const PIT_FREQUENCY_HZ: u64 = 1_193_182;

/// Bornes de plausibilité de la fréquence LAPIC *après* le diviseur.
/// Couvre un cristal de 19,2 MHz / 16 (1,2 MHz) jusqu'à QEMU (1 GHz / 16) et
/// au-delà avec une marge, tout en rejetant une mesure corrompue.
pub const MIN_LAPIC_HZ: u64 = 500_000;
pub const MAX_LAPIC_HZ: u64 = 1_000_000_000;

/// Feuille CPUID « generic timing » de VMware, reprise par KVM.
pub const HYPERVISOR_TIMING_LEAF: u32 = 0x4000_0010;

pub trait PitPort {
    /// État de la sortie OUT2 (bit 5 du port 0x61).
    fn out2_high(&mut self) -> bool;
    /// Latche puis lit le compteur du canal 2.
    fn latched_count(&mut self) -> u16;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitError {
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationError {
    LapicDidNotAdvance,
    NoPitTicks,
    ImplausibleFrequency,
    InvalidInitialCount,
}

impl CalibrationError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LapicDidNotAdvance => "Local APIC timer did not advance",
            Self::NoPitTicks => "empty PIT calibration window",
            Self::ImplausibleFrequency => "implausible Local APIC timer frequency",
            Self::InvalidInitialCount => "invalid Local APIC periodic initial count",
        }
    }
}

/// Attend le compte terminal d'un canal 2 programmé en mode 0.
///
/// En mode 0, OUT2 passe bas à l'écriture du mot de commande et ne remonte
/// qu'au compte terminal, où il reste haut. OUT2 haut suffit donc : exiger
/// d'avoir aussi observé l'état bas manque la fin quand la première lecture
/// arrive après le compte terminal (sortie VM lente). Renvoie le nombre de
/// lectures effectuées.
pub fn wait_for_terminal_count<P: PitPort + ?Sized>(
    port: &mut P,
    max_polls: usize,
) -> Result<usize, PitError> {
    for poll in 1..=max_polls {
        if port.out2_high() {
            return Ok(poll);
        }
        core::hint::spin_loop();
    }
    Err(PitError::Timeout)
}

/// Ticks PIT réellement écoulés depuis le chargement de `initial`, une fois le
/// compte terminal atteint : après 0, le compteur repasse à 0xFFFF et continue
/// de décompter, ce qui mesure le dépassement (jusqu'à ~55 ms).
pub const fn elapsed_pit_ticks(initial: u16, latched_after_terminal: u16) -> u32 {
    initial as u32 + (0x1_0000 - latched_after_terminal as u32) % 0x1_0000
}

/// Compte PIT pour un délai minimal de `microseconds`, borné à [1, 0xFFFF].
pub const fn pit_count_for_us(microseconds: u32) -> u16 {
    let count = PIT_FREQUENCY_HZ * microseconds as u64 / 1_000_000;
    if count == 0 {
        1
    } else if count > u16::MAX as u64 {
        u16::MAX
    } else {
        count as u16
    }
}

pub const fn lapic_frequency_is_plausible(hz: u64) -> bool {
    hz >= MIN_LAPIC_HZ && hz <= MAX_LAPIC_HZ
}

/// Fréquence LAPIC (après diviseur) à partir des ticks LAPIC écoulés pendant
/// `pit_ticks` ticks PIT.
pub fn lapic_frequency(lapic_elapsed: u32, pit_ticks: u32) -> Result<u64, CalibrationError> {
    if lapic_elapsed == 0 {
        return Err(CalibrationError::LapicDidNotAdvance);
    }
    if pit_ticks == 0 {
        return Err(CalibrationError::NoPitTicks);
    }

    let hz = lapic_elapsed as u64 * PIT_FREQUENCY_HZ / pit_ticks as u64;
    if !lapic_frequency_is_plausible(hz) {
        return Err(CalibrationError::ImplausibleFrequency);
    }
    Ok(hz)
}

pub fn periodic_initial_count(lapic_hz: u64, target_hz: u32) -> Result<u32, CalibrationError> {
    if target_hz == 0 {
        return Err(CalibrationError::InvalidInitialCount);
    }
    match lapic_hz / target_hz as u64 {
        0 => Err(CalibrationError::InvalidInitialCount),
        count => u32::try_from(count).map_err(|_| CalibrationError::InvalidInitialCount),
    }
}

/// Fréquence LAPIC (après diviseur) annoncée par l'hyperviseur via la feuille
/// 0x40000010 (EBX = bus LAPIC en kHz), si elle existe et est plausible.
pub fn hypervisor_lapic_hz(
    max_hypervisor_leaf: u32,
    timing_ebx_khz: u32,
    divide: u32,
) -> Option<u64> {
    if max_hypervisor_leaf < HYPERVISOR_TIMING_LEAF || timing_ebx_khz == 0 || divide == 0 {
        return None;
    }
    let hz = timing_ebx_khz as u64 * 1_000 / divide as u64;
    lapic_frequency_is_plausible(hz).then_some(hz)
}
