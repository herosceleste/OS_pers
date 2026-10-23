//! Préemption par timer (K004-04), sans accès matériel.
//!
//! Le noyau appelle [`SliceClock::on_tick`] depuis l'IRQ timer pour la tâche
//! courante ; ce module décide si sa tranche est épuisée et vérifie après coup
//! que la trace des tranches respecte la rotation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreemptError {
    ZeroQuantum,
}

impl PreemptError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroQuantum => "preemption quantum must be at least one tick",
        }
    }
}

/// Durée d'une tranche, en ticks timer (au moins un).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantum(u32);

impl Quantum {
    pub const fn new(ticks: u32) -> Result<Self, PreemptError> {
        if ticks == 0 {
            return Err(PreemptError::ZeroQuantum);
        }
        Ok(Self(ticks))
    }

    pub const fn ticks(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickDecision {
    Continue,
    Preempt,
}

/// Ticks restants dans la tranche de la tâche courante.
#[derive(Clone, Copy, Debug)]
pub struct SliceClock {
    quantum: Quantum,
    remaining: u32,
}

impl SliceClock {
    pub const fn new(quantum: Quantum) -> Self {
        Self {
            quantum,
            remaining: quantum.ticks(),
        }
    }

    pub const fn remaining(&self) -> u32 {
        self.remaining
    }

    /// Décompte un tick. Une tranche épuisée redemande la préemption à chaque
    /// tick tant que le scheduler ne l'a pas remise à zéro par [`Self::reset`].
    pub fn on_tick(&mut self) -> TickDecision {
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            TickDecision::Preempt
        } else {
            TickDecision::Continue
        }
    }

    /// Donne une tranche pleine, à appeler quand une tâche prend le CPU.
    pub fn reset(&mut self) {
        self.remaining = self.quantum.ticks();
    }
}

/// Vrai si `trace` suit la rotation de `tasks` depuis la première tâche, sans
/// saut ni répétition, quelle que soit sa longueur.
pub fn is_rotation(trace: &[u64], tasks: &[u64]) -> bool {
    if trace.is_empty() || tasks.is_empty() {
        return false;
    }
    trace
        .iter()
        .enumerate()
        .all(|(index, &task)| task == tasks[index % tasks.len()])
}

/// Nombre de tranches de la tâche la moins servie de `tasks` dans `trace`.
pub fn min_slices(trace: &[u64], tasks: &[u64]) -> u64 {
    tasks
        .iter()
        .map(|&task| trace.iter().filter(|&&entry| entry == task).count() as u64)
        .min()
        .unwrap_or(0)
}
