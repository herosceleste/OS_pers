//! Scheduler per-CPU (K004-07), sans état global ni allocation.
//!
//! Place une tâche sur un CPU selon son affinité et vérifie qu'une trace
//! d'exécution multi-CPU respecte l'affinité, la rotation par CPU et la
//! présence de l'idle task de chaque CPU.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    EmptyAffinity,
    NoOnlineCpu,
}

impl PlacementError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyAffinity => "task affinity mask is empty",
            Self::NoOnlineCpu => "no online CPU in the task affinity mask",
        }
    }
}

/// Plus petit CPU en ligne de l'affinité.
pub fn place(affinity: u64, online_mask: u64) -> Result<usize, PlacementError> {
    if affinity == 0 {
        return Err(PlacementError::EmptyAffinity);
    }
    let candidates = affinity & online_mask;
    if candidates == 0 {
        return Err(PlacementError::NoOnlineCpu);
    }
    Ok(candidates.trailing_zeros() as usize)
}

/// Un passage d'une tâche sur un CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    pub cpu: u32,
    pub task: u64,
}

/// Ce que la trace doit montrer : pour chaque CPU, ses tâches dans l'ordre de
/// sa run queue, son idle task, et le nombre d'itérations de chaque tâche.
pub struct Expectation<'a> {
    pub tasks: &'a [&'a [u64]],
    pub idle: &'a [u64],
    pub iterations: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceError {
    UnknownCpu { cpu: u32 },
    UnknownTask { task: u64 },
    WrongCpu { task: u64, cpu: u32 },
    OrderBroken { cpu: u32 },
    IdleMissing { cpu: u32 },
    RunCount { task: u64 },
}

/// Vérifie, dans cet ordre :
/// 1. chaque passage a lieu sur un CPU connu, pour une tâche connue, et sur le
///    CPU auquel la tâche (ou l'idle task) appartient ;
/// 2. sur chaque CPU, les passages hors idle forment exactement `iterations`
///    tours de ses tâches, dans l'ordre ;
/// 3. l'idle task de chaque CPU y a tourné au moins une fois.
///
/// L'entrelacement entre CPU est libre.
pub fn check_trace(trace: &[TraceEntry], expectation: &Expectation<'_>) -> Result<(), TraceError> {
    let cpus = expectation.tasks.len();

    for entry in trace {
        if entry.cpu as usize >= cpus {
            return Err(TraceError::UnknownCpu { cpu: entry.cpu });
        }
        let owner = owner_of(entry.task, expectation)
            .ok_or(TraceError::UnknownTask { task: entry.task })?;
        if owner != entry.cpu as usize {
            return Err(TraceError::WrongCpu {
                task: entry.task,
                cpu: entry.cpu,
            });
        }
    }

    for cpu in 0..cpus {
        let tasks = expectation.tasks[cpu];
        let idle = expectation.idle.get(cpu).copied();
        let mut runs = 0u64;
        let mut idle_seen = false;

        for entry in trace.iter().filter(|entry| entry.cpu as usize == cpu) {
            if Some(entry.task) == idle {
                idle_seen = true;
                continue;
            }
            let expected = tasks.get((runs % tasks.len().max(1) as u64) as usize);
            if expected != Some(&entry.task) {
                return Err(TraceError::OrderBroken { cpu: cpu as u32 });
            }
            runs += 1;
        }

        if runs != tasks.len() as u64 * expectation.iterations {
            return Err(TraceError::OrderBroken { cpu: cpu as u32 });
        }
        if !idle_seen {
            return Err(TraceError::IdleMissing { cpu: cpu as u32 });
        }
    }

    Ok(())
}

fn owner_of(task: u64, expectation: &Expectation<'_>) -> Option<usize> {
    if let Some(cpu) = expectation.idle.iter().position(|&idle| idle == task) {
        return Some(cpu);
    }
    expectation
        .tasks
        .iter()
        .position(|tasks| tasks.contains(&task))
}

/// Un IPI de rescheduling n'est utile que vers un autre CPU : le CPU courant
/// voit sa propre file au prochain passage dans le scheduler.
pub const fn should_send_ipi(target_cpu: usize, this_cpu: usize) -> bool {
    target_cpu != this_cpu
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Migration {
    /// La tâche reste sur son CPU, toujours autorisé.
    Stay(usize),
    /// Le CPU courant sort de l'affinité : la tâche change de file.
    Move { from: usize, to: usize },
    /// La tâche n'était dans aucune file.
    Place(usize),
}

/// Décision après un changement d'affinité de la tâche placée sur `current`.
pub fn migrate(
    current: Option<usize>,
    affinity: u64,
    online_mask: u64,
) -> Result<Migration, PlacementError> {
    if affinity == 0 {
        return Err(PlacementError::EmptyAffinity);
    }
    if let Some(cpu) = current
        && cpu < 64
        && affinity & online_mask & (1 << cpu) != 0
    {
        return Ok(Migration::Stay(cpu));
    }
    let to = place(affinity, online_mask)?;
    Ok(match current {
        Some(from) => Migration::Move { from, to },
        None => Migration::Place(to),
    })
}

/// CPU et nombre de passages attendus pour une tâche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub task: u64,
    pub cpu: u32,
    pub runs: u64,
}

/// Chaque passage concerne une tâche attendue, sur son CPU ; puis chaque tâche
/// a exactement le nombre de passages prévu.
pub fn check_placement(trace: &[TraceEntry], expected: &[Placed]) -> Result<(), TraceError> {
    for entry in trace {
        let placed = expected
            .iter()
            .find(|placed| placed.task == entry.task)
            .ok_or(TraceError::UnknownTask { task: entry.task })?;
        if placed.cpu != entry.cpu {
            return Err(TraceError::WrongCpu {
                task: entry.task,
                cpu: entry.cpu,
            });
        }
    }
    for placed in expected {
        let runs = trace
            .iter()
            .filter(|entry| entry.task == placed.task)
            .count() as u64;
        if runs != placed.runs {
            return Err(TraceError::RunCount { task: placed.task });
        }
    }
    Ok(())
}
