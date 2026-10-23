//! Tests hôte du scheduler per-CPU (K004-07) : placement par affinité et
//! contrôle d'une trace d'exécution multi-CPU.

use elyon_core::percpu_sched::{self, Expectation, PlacementError, TraceEntry, TraceError};

const CPU0: u64 = 1 << 0;
const CPU2: u64 = 1 << 2;
const ALL4: u64 = 0b1111;

// --- Placement --------------------------------------------------------------------

#[test]
fn task_is_placed_on_the_lowest_online_cpu_of_its_affinity() {
    assert_eq!(percpu_sched::place(0b1100, ALL4), Ok(2));
    assert_eq!(percpu_sched::place(CPU0, ALL4), Ok(0));
}

#[test]
fn offline_cpus_are_skipped() {
    assert_eq!(percpu_sched::place(0b0110, 0b1101), Ok(2));
}

#[test]
fn empty_affinity_is_rejected() {
    assert_eq!(
        percpu_sched::place(0, ALL4),
        Err(PlacementError::EmptyAffinity)
    );
}

#[test]
fn affinity_without_any_online_cpu_is_rejected() {
    assert_eq!(
        percpu_sched::place(0b1000, 0b0111),
        Err(PlacementError::NoOnlineCpu)
    );
}

// --- Trace multi-CPU --------------------------------------------------------------

// CPU 0 : tâches 10, 11 ; CPU 1 : 20, 21 ; idle 100 et 101 ; deux itérations.
const CPU0_TASKS: [u64; 2] = [10, 11];
const CPU1_TASKS: [u64; 2] = [20, 21];
const IDLE: [u64; 2] = [100, 101];

fn expectation() -> Expectation<'static> {
    Expectation {
        tasks: &[&CPU0_TASKS, &CPU1_TASKS],
        idle: &IDLE,
        iterations: 2,
    }
}

fn e(cpu: u32, task: u64) -> TraceEntry {
    TraceEntry { cpu, task }
}

/// Trace valide où les deux CPU s'entrelacent arbitrairement.
fn valid_trace() -> Vec<TraceEntry> {
    vec![
        e(0, 10),
        e(1, 20),
        e(1, 21),
        e(0, 11),
        e(1, 20),
        e(0, 10),
        e(1, 21),
        e(1, 101),
        e(0, 11),
        e(0, 100),
    ]
}

#[test]
fn interleaved_per_cpu_round_robin_with_idle_is_accepted() {
    assert_eq!(
        percpu_sched::check_trace(&valid_trace(), &expectation()),
        Ok(())
    );
}

#[test]
fn a_task_running_on_another_cpu_is_detected() {
    let mut trace = valid_trace();
    trace[1] = e(0, 20);
    assert_eq!(
        percpu_sched::check_trace(&trace, &expectation()),
        Err(TraceError::WrongCpu { task: 20, cpu: 0 })
    );
}

#[test]
fn broken_round_robin_on_one_cpu_is_detected() {
    let trace = vec![
        e(0, 10),
        e(0, 10),
        e(0, 11),
        e(0, 11),
        e(0, 100),
        e(1, 20),
        e(1, 21),
        e(1, 20),
        e(1, 21),
        e(1, 101),
    ];
    assert_eq!(
        percpu_sched::check_trace(&trace, &expectation()),
        Err(TraceError::OrderBroken { cpu: 0 })
    );
}

#[test]
fn missing_runs_on_a_cpu_are_detected() {
    let mut trace = valid_trace();
    trace.retain(|entry| *entry != e(1, 21));
    assert_eq!(
        percpu_sched::check_trace(&trace, &expectation()),
        Err(TraceError::OrderBroken { cpu: 1 })
    );
}

#[test]
fn a_cpu_whose_idle_task_never_ran_is_detected() {
    let mut trace = valid_trace();
    trace.retain(|entry| entry.task != 101);
    assert_eq!(
        percpu_sched::check_trace(&trace, &expectation()),
        Err(TraceError::IdleMissing { cpu: 1 })
    );
}

#[test]
fn an_idle_task_on_the_wrong_cpu_is_detected() {
    let mut trace = valid_trace();
    trace.push(e(0, 101));
    assert_eq!(
        percpu_sched::check_trace(&trace, &expectation()),
        Err(TraceError::WrongCpu { task: 101, cpu: 0 })
    );
}

#[test]
fn an_unknown_task_or_cpu_is_detected() {
    let mut unknown_task = valid_trace();
    unknown_task.push(e(0, 999));
    assert_eq!(
        percpu_sched::check_trace(&unknown_task, &expectation()),
        Err(TraceError::UnknownTask { task: 999 })
    );

    let mut unknown_cpu = valid_trace();
    unknown_cpu.push(e(7, 10));
    assert_eq!(
        percpu_sched::check_trace(&unknown_cpu, &expectation()),
        Err(TraceError::UnknownCpu { cpu: 7 })
    );
}

#[test]
fn idle_may_run_several_times_on_its_own_cpu() {
    let mut trace = valid_trace();
    trace.push(e(0, 100));
    trace.push(e(1, 101));
    assert_eq!(percpu_sched::check_trace(&trace, &expectation()), Ok(()));
}

#[test]
fn placement_of_the_qualification_tasks_matches_their_single_cpu_affinity() {
    for cpu in 0..4 {
        assert_eq!(percpu_sched::place(1 << cpu, ALL4), Ok(cpu));
    }
    assert_eq!(percpu_sched::place(CPU2, ALL4), Ok(2));
}
