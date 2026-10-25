//! Ordre des verrous du noyau (K004-10), sans état global.
//!
//! Chaque verrou appartient à une classe de rang fixe ; un CPU ne prend un
//! verrou que si tous ceux qu'il tient sont de rang strictement inférieur. Deux
//! CPU qui respectent cet ordre ne peuvent pas s'interbloquer. Le noyau garde
//! par CPU l'ensemble des classes tenues et le vérifie à chaque prise et avant
//! chaque changement de contexte.

/// Classes de verrous, par rang croissant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LockClass {
    /// Allocateur de pages physiques.
    Pmm = 0,
    /// Run queue d'un CPU ; jamais deux à la fois.
    RunQueue = 1,
    /// File temporelle des tâches endormies.
    TimerQueue = 2,
    /// Données partagées entre tâches ; aucun verrou noyau n'est pris dessous.
    Data = 3,
}

impl LockClass {
    pub const ALL: [Self; 4] = [Self::Pmm, Self::RunQueue, Self::TimerQueue, Self::Data];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pmm => "pmm",
            Self::RunQueue => "run_queue",
            Self::TimerQueue => "timer_queue",
            Self::Data => "data",
        }
    }

    const fn bit(self) -> u32 {
        1 << self as u8
    }
}

/// Ensemble des classes tenues par un CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldLocks(u32);

impl HeldLocks {
    pub const NONE: Self = Self(0);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, class: LockClass) -> bool {
        self.0 & class.bit() != 0
    }

    /// Classe tenue de rang le plus élevé.
    fn highest(self) -> Option<LockClass> {
        LockClass::ALL
            .iter()
            .rev()
            .copied()
            .find(|&class| self.contains(class))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockOrderError {
    /// Prise d'un verrou de rang inférieur à un verrou déjà tenu.
    Inversion {
        held: LockClass,
        acquiring: LockClass,
    },
    /// Prise d'une classe déjà tenue.
    Reentrant(LockClass),
    /// Libération d'une classe non tenue.
    NotHeld(LockClass),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchError {
    LockHeld(LockClass),
    InterruptsEnabled,
}

/// Vérifie la prise de `class` et retourne le nouvel ensemble tenu.
pub fn check_acquire(held: HeldLocks, class: LockClass) -> Result<HeldLocks, LockOrderError> {
    if held.contains(class) {
        return Err(LockOrderError::Reentrant(class));
    }
    if let Some(highest) = held.highest()
        && highest as u8 > class as u8
    {
        return Err(LockOrderError::Inversion {
            held: highest,
            acquiring: class,
        });
    }
    Ok(HeldLocks(held.0 | class.bit()))
}

pub fn release(held: HeldLocks, class: LockClass) -> Result<HeldLocks, LockOrderError> {
    if !held.contains(class) {
        return Err(LockOrderError::NotHeld(class));
    }
    Ok(HeldLocks(held.0 & !class.bit()))
}

/// Un changement de contexte ne se fait qu'IRQ masquées et sans verrou tenu : un
/// verrou emporté par la tâche sortante bloquerait tout CPU qui le demande.
pub fn check_switch(held: HeldLocks, interrupts_enabled: bool) -> Result<(), SwitchError> {
    if let Some(class) = held.highest() {
        return Err(SwitchError::LockHeld(class));
    }
    if interrupts_enabled {
        return Err(SwitchError::InterruptsEnabled);
    }
    Ok(())
}
