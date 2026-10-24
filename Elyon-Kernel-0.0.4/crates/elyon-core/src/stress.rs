//! Contrôle du stress SMP (K004-11), sans état global.
//!
//! Le noyau mesure pendant le stress ; ce module décide si la mesure prouve
//! les invariants : toutes les tâches créées ont terminé et ont été récupérées,
//! sans fuite de pages ni de slots, sans exécution simultanée, sans corruption,
//! sans famine, avec un volume minimal de changements de contexte.

/// Mesure d'un stress, relevée par le BSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StressReport {
    pub cpus: u32,
    pub created: u64,
    pub exited: u64,
    pub reaped: u64,
    pub switches: u64,
    /// Plus grand nombre de tâches RUNNING vues sur un même CPU à un switch ;
    /// la garde contre une tâche sur deux CPU est la réclamation de `on_cpu`.
    pub running_max: u64,
    /// Registres ou état FPU retrouvés altérés après une préemption.
    pub corruptions: u64,
    pub canary_failures: u64,
    pub free_frames_before: u64,
    pub free_frames_after: u64,
    pub free_slots_before: u64,
    pub free_slots_after: u64,
    /// Plus petite progression d'une tâche, en tours achevés.
    pub min_progress: u64,
    /// Plus longue attente d'une tâche prête entre deux exécutions : nombre de
    /// changements de contexte de son CPU entre les deux. En Round-Robin, il est
    /// borné par la longueur de la file ; une tâche affamée en voit bien plus.
    pub max_wait_switches: u64,
    pub idle_running_aps: u64,
}

/// Seuils exigés par la qualification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StressLimits {
    pub cpus: u32,
    pub expected_tasks: u64,
    pub min_switches: u64,
    pub min_progress: u64,
    pub max_wait_switches: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressError {
    CpuCount,
    TaskCount,
    NotAllExited,
    NotAllReaped,
    TooFewSwitches,
    Concurrency,
    Corruption,
    Canary,
    FrameLeak,
    SlotLeak,
    Starvation,
    IdleNotReached,
}

impl StressError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CpuCount => "stress ran on an unexpected number of CPUs",
            Self::TaskCount => "stress did not create the expected number of tasks",
            Self::NotAllExited => "a stress task never exited",
            Self::NotAllReaped => "a dead stress task was not reaped",
            Self::TooFewSwitches => "too few context switches under stress",
            Self::Concurrency => "a task ran on two CPUs at once",
            Self::Corruption => "registers or FPU state corrupted under stress",
            Self::Canary => "a stress task stack canary was overwritten",
            Self::FrameLeak => "free frames not restored after reaping",
            Self::SlotLeak => "task slots not restored after reaping",
            Self::Starvation => "a stress task was starved",
            Self::IdleNotReached => "an AP did not return to its idle task",
        }
    }
}

/// Vérifie la mesure contre les seuils ; la première violation trouvée est
/// rendue. Les pages comme les slots doivent revenir exactement à leur valeur
/// de départ : plus signalerait une double libération, moins une fuite.
pub fn check_stress(report: &StressReport, limits: &StressLimits) -> Result<(), StressError> {
    if report.cpus != limits.cpus {
        return Err(StressError::CpuCount);
    }
    if report.created != limits.expected_tasks {
        return Err(StressError::TaskCount);
    }
    if report.exited != report.created {
        return Err(StressError::NotAllExited);
    }
    if report.reaped != report.created {
        return Err(StressError::NotAllReaped);
    }
    if report.switches < limits.min_switches {
        return Err(StressError::TooFewSwitches);
    }
    if report.running_max != 1 {
        return Err(StressError::Concurrency);
    }
    if report.corruptions != 0 {
        return Err(StressError::Corruption);
    }
    if report.canary_failures != 0 {
        return Err(StressError::Canary);
    }
    if report.free_frames_after != report.free_frames_before {
        return Err(StressError::FrameLeak);
    }
    if report.free_slots_after != report.free_slots_before {
        return Err(StressError::SlotLeak);
    }
    if report.min_progress < limits.min_progress
        || report.max_wait_switches > limits.max_wait_switches
    {
        return Err(StressError::Starvation);
    }
    if report.idle_running_aps != u64::from(limits.cpus.saturating_sub(1)) {
        return Err(StressError::IdleNotReached);
    }
    Ok(())
}
