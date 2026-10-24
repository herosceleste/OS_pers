//! File temporelle des tâches endormies (K004-09), sans état global ni allocation.
//!
//! Le noyau la protège par un verrou et la vide à chaque tick ; ce module garde
//! les échéances triées, calcule une échéance qui ne réveille jamais en avance
//! et vérifie une trace de sommeils.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerQueueError {
    Full,
    Duplicate,
    InvalidTask,
}

impl TimerQueueError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "timer queue is full",
            Self::Duplicate => "task already has a pending deadline",
            Self::InvalidTask => "task id 0 cannot sleep",
        }
    }
}

/// `N` échéances `(tâche, tick)` triées par tick ; à tick égal, ordre d'arrivée.
pub struct TimerQueue<const N: usize> {
    entries: [(u64, u64); N],
    len: usize,
}

impl<const N: usize> TimerQueue<N> {
    pub const fn new() -> Self {
        Self {
            entries: [(0, 0); N],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, task: u64) -> bool {
        self.entries[..self.len]
            .iter()
            .any(|&(entry, _)| entry == task)
    }

    /// Vrai si une échéance en attente satisfait `predicate` (K004-11 : une
    /// tâche n'est récupérée que si aucune échéance ne la désigne).
    pub fn any(&self, predicate: impl Fn(u64) -> bool) -> bool {
        self.entries[..self.len]
            .iter()
            .any(|&(entry, _)| predicate(entry))
    }

    pub fn next_deadline(&self) -> Option<u64> {
        (self.len > 0).then(|| self.entries[0].1)
    }

    pub fn insert(&mut self, task: u64, deadline: u64) -> Result<(), TimerQueueError> {
        if task == 0 {
            return Err(TimerQueueError::InvalidTask);
        }
        if self.contains(task) {
            return Err(TimerQueueError::Duplicate);
        }
        if self.len == N {
            return Err(TimerQueueError::Full);
        }
        // Après toutes les échéances inférieures ou égales : FIFO à tick égal.
        let at = self.entries[..self.len]
            .iter()
            .position(|&(_, tick)| tick > deadline)
            .unwrap_or(self.len);
        self.entries.copy_within(at..self.len, at + 1);
        self.entries[at] = (task, deadline);
        self.len += 1;
        Ok(())
    }

    /// Retire l'échéance de `task` (réveil explicite) ; faux si elle n'en a pas.
    pub fn cancel(&mut self, task: u64) -> bool {
        match self.entries[..self.len]
            .iter()
            .position(|&(entry, _)| entry == task)
        {
            Some(at) => {
                self.remove_at(at);
                true
            }
            None => false,
        }
    }

    /// Tâche d'échéance la plus proche si elle est atteinte à `now`.
    pub fn pop_expired(&mut self, now: u64) -> Option<u64> {
        if self.len == 0 || self.entries[0].1 > now {
            return None;
        }
        let task = self.entries[0].0;
        self.remove_at(0);
        Some(task)
    }

    fn remove_at(&mut self, at: usize) {
        self.entries.copy_within(at + 1..self.len, at);
        self.len -= 1;
        self.entries[self.len] = (0, 0);
    }
}

impl<const N: usize> Default for TimerQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Tick d'échéance d'un sommeil de `ms` millisecondes commencé au tick `now`.
///
/// La durée est arrondie au tick supérieur, plus un tick : le tick courant est
/// déjà entamé, et sans cette marge le réveil pourrait survenir jusqu'à une
/// période trop tôt. Une durée nulle expire immédiatement.
pub fn deadline_for(now: u64, ms: u64, hz: u32) -> u64 {
    if ms == 0 {
        return now;
    }
    let ticks = ms
        .checked_mul(hz as u64)
        .map_or(u64::MAX, |scaled| scaled.div_ceil(1_000));
    now.saturating_add(ticks).saturating_add(1)
}

/// Un sommeil observé : échéance, tick du réveil, nombre de réveils, et s'il a
/// été interrompu par un réveil explicite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepRecord {
    pub task: u64,
    pub deadline: u64,
    pub woke_at: u64,
    pub wakes: u32,
    pub explicit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepError {
    WakeCount { task: u64 },
    Early { task: u64 },
    Late { task: u64 },
    ExplicitNotEarly { task: u64 },
    OrderBroken { task: u64 },
}

/// Chaque sommeil a exactement un réveil ; un réveil par le timer survient au
/// plus `max_late` ticks après l'échéance, jamais avant, et les réveils par le
/// timer suivent l'ordre des échéances ; un réveil explicite précède l'échéance.
pub fn check_sleep(records: &[SleepRecord], max_late: u64) -> Result<(), SleepError> {
    for record in records {
        let task = record.task;
        if record.wakes != 1 {
            return Err(SleepError::WakeCount { task });
        }
        if record.explicit {
            if record.woke_at >= record.deadline {
                return Err(SleepError::ExplicitNotEarly { task });
            }
            continue;
        }
        if record.woke_at < record.deadline {
            return Err(SleepError::Early { task });
        }
        if record.woke_at - record.deadline > max_late {
            return Err(SleepError::Late { task });
        }
    }
    // Ordre : une échéance plus tardive ne se réveille jamais avant une plus proche.
    for later in records.iter().filter(|record| !record.explicit) {
        let broken = records
            .iter()
            .filter(|record| !record.explicit)
            .any(|earlier| earlier.deadline < later.deadline && earlier.woke_at > later.woke_at);
        if broken {
            return Err(SleepError::OrderBroken { task: later.task });
        }
    }
    Ok(())
}
