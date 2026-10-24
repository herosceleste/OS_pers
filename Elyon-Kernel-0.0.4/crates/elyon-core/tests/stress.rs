//! Tests hôte du contrôle de stress SMP (K004-11) : chaque invariant violé est
//! refusé avec son erreur, un rapport sain est accepté.

use elyon_core::stress::{self, StressError, StressLimits, StressReport};
use elyon_core::timer_queue::TimerQueue;

fn limits() -> StressLimits {
    StressLimits {
        cpus: 4,
        expected_tasks: 1024,
        min_switches: 100_000,
        min_progress: 50,
        max_wait_ticks: 64,
    }
}

fn healthy() -> StressReport {
    StressReport {
        cpus: 4,
        created: 1024,
        exited: 1024,
        reaped: 1024,
        switches: 150_000,
        running_max: 1,
        corruptions: 0,
        canary_failures: 0,
        free_frames_before: 200_000,
        free_frames_after: 200_000,
        free_slots_before: 190,
        free_slots_after: 190,
        min_progress: 60,
        max_wait_ticks: 40,
        idle_running_aps: 3,
    }
}

#[test]
fn healthy_report_is_accepted() {
    assert_eq!(stress::check_stress(&healthy(), &limits()), Ok(()));
}

#[test]
fn wrong_cpu_count_is_refused() {
    let report = StressReport {
        cpus: 2,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::CpuCount)
    );
}

#[test]
fn missing_tasks_are_refused() {
    let report = StressReport {
        created: 1000,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::TaskCount)
    );
}

#[test]
fn a_task_that_never_exits_is_refused() {
    let report = StressReport {
        exited: 1023,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::NotAllExited)
    );
}

#[test]
fn a_task_left_unreaped_is_refused() {
    let report = StressReport {
        reaped: 1023,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::NotAllReaped)
    );
}

#[test]
fn too_few_switches_are_refused() {
    let report = StressReport {
        switches: 99_999,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::TooFewSwitches)
    );
}

#[test]
fn a_task_running_twice_at_once_is_refused() {
    let report = StressReport {
        running_max: 2,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::Concurrency)
    );
}

#[test]
fn register_or_fpu_corruption_is_refused() {
    let report = StressReport {
        corruptions: 1,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::Corruption)
    );
}

#[test]
fn a_broken_canary_is_refused() {
    let report = StressReport {
        canary_failures: 1,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::Canary)
    );
}

#[test]
fn leaked_frames_are_refused() {
    let report = StressReport {
        free_frames_after: 199_984,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::FrameLeak)
    );
}

#[test]
fn frames_freed_twice_are_refused_as_well() {
    let report = StressReport {
        free_frames_after: 200_016,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::FrameLeak)
    );
}

#[test]
fn leaked_task_slots_are_refused() {
    let report = StressReport {
        free_slots_after: 189,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::SlotLeak)
    );
}

#[test]
fn a_starved_task_is_refused_by_progress() {
    let report = StressReport {
        min_progress: 49,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::Starvation)
    );
}

#[test]
fn a_starved_task_is_refused_by_wait() {
    let report = StressReport {
        max_wait_ticks: 65,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::Starvation)
    );
}

#[test]
fn an_ap_that_does_not_return_to_idle_is_refused() {
    let report = StressReport {
        idle_running_aps: 2,
        ..healthy()
    };
    assert_eq!(
        stress::check_stress(&report, &limits()),
        Err(StressError::IdleNotReached)
    );
}

#[test]
fn every_error_has_a_message() {
    for error in [
        StressError::CpuCount,
        StressError::TaskCount,
        StressError::NotAllExited,
        StressError::NotAllReaped,
        StressError::TooFewSwitches,
        StressError::Concurrency,
        StressError::Corruption,
        StressError::Canary,
        StressError::FrameLeak,
        StressError::SlotLeak,
        StressError::Starvation,
        StressError::IdleNotReached,
    ] {
        assert!(!error.as_str().is_empty());
    }
}

// --- Recherche dans la file temporelle --------------------------------------

#[test]
fn timer_queue_any_finds_a_matching_key() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert((7 << 32) | 42, 10).unwrap();
    queue.insert((8 << 32) | 43, 20).unwrap();
    assert!(queue.any(|key| key & 0xFFFF_FFFF == 43));
    assert!(!queue.any(|key| key & 0xFFFF_FFFF == 44));
}

#[test]
fn timer_queue_any_is_false_once_the_key_expired() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert((7 << 32) | 42, 10).unwrap();
    assert_eq!(queue.pop_expired(10), Some((7 << 32) | 42));
    assert!(!queue.any(|key| key & 0xFFFF_FFFF == 42));
}
