//! Run queue Round-Robin coopérative (K004-03), sans état global ni allocation.
//!
//! Le noyau garde la file et les contextes ; ce module décide quelle tâche
//! reprend la main et vérifie qu'une trace d'exécution respecte la rotation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    Full,
    Duplicate,
    InvalidTask,
}

impl QueueError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "run queue is full",
            Self::Duplicate => "task is already in the run queue",
            Self::InvalidTask => "task id 0 cannot be queued",
        }
    }
}

/// File FIFO circulaire de `N` identifiants de tâches non nuls.
pub struct RunQueue<const N: usize> {
    slots: [u64; N],
    head: usize,
    len: usize,
}

impl<const N: usize> RunQueue<N> {
    pub const fn new() -> Self {
        Self {
            slots: [0; N],
            head: 0,
            len: 0,
        }
    }

    pub const fn capacity() -> usize {
        N
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, task: u64) -> bool {
        (0..self.len).any(|offset| self.slots[(self.head + offset) % N] == task)
    }

    pub fn push_back(&mut self, task: u64) -> Result<(), QueueError> {
        if task == 0 {
            return Err(QueueError::InvalidTask);
        }
        if self.contains(task) {
            return Err(QueueError::Duplicate);
        }
        if self.len == N {
            return Err(QueueError::Full);
        }
        self.slots[(self.head + self.len) % N] = task;
        self.len += 1;
        Ok(())
    }

    /// Retire `task` de la file (migration) en conservant l'ordre des autres.
    pub fn remove(&mut self, task: u64) -> bool {
        if !self.contains(task) {
            return false;
        }
        for _ in 0..self.len {
            if let Some(entry) = self.pop_front()
                && entry != task
            {
                // La place libérée par pop_front garantit que push_back réussit.
                let _ = self.push_back(entry);
            }
        }
        true
    }

    pub fn pop_front(&mut self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        let task = self.slots[self.head];
        self.slots[self.head] = 0;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(task)
    }
}

impl<const N: usize> Default for RunQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// `yield()` : la tâche courante repasse en fin de file, la tête reprend la main.
/// Seule dans la file, la tâche courante est reprise immédiatement.
pub fn yield_next<const N: usize>(
    queue: &mut RunQueue<N>,
    current: u64,
) -> Result<u64, QueueError> {
    queue.push_back(current)?;
    // La file contient au moins `current` : pop_front ne peut pas échouer.
    Ok(queue.pop_front().unwrap_or(current))
}

/// Sortie de la tâche courante : elle n'est pas remise dans la file.
/// `None` signifie qu'aucune tâche n'est prête et que le BSP reprend la main.
pub fn exit_next<const N: usize>(queue: &mut RunQueue<N>) -> Option<u64> {
    queue.pop_front()
}

/// Vrai si `trace` est exactement `iterations` tours de `tasks`, dans l'ordre.
pub fn is_round_robin(trace: &[u64], tasks: &[u64], iterations: u64) -> bool {
    if tasks.is_empty() || iterations == 0 {
        return false;
    }
    if trace.len() as u64 != tasks.len() as u64 * iterations {
        return false;
    }
    trace
        .iter()
        .enumerate()
        .all(|(index, &task)| task == tasks[index % tasks.len()])
}
