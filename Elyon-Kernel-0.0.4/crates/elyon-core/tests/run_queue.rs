//! Tests hôte de la run queue Round-Robin coopérative (K004-03).

use elyon_core::run_queue::{self, QueueError, RunQueue};

const A: u64 = 3;
const B: u64 = 4;
const C: u64 = 5;

fn queue_of<const N: usize>(tasks: &[u64]) -> RunQueue<N> {
    let mut queue = RunQueue::<N>::new();
    for &task in tasks {
        queue.push_back(task).unwrap();
    }
    queue
}

// --- File FIFO --------------------------------------------------------------------

#[test]
fn new_queue_is_empty_with_its_capacity() {
    let queue = RunQueue::<4>::new();
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert_eq!(RunQueue::<4>::capacity(), 4);
}

#[test]
fn tasks_leave_in_fifo_order() {
    let mut queue = queue_of::<4>(&[A, B, C]);
    assert_eq!(queue.pop_front(), Some(A));
    assert_eq!(queue.pop_front(), Some(B));
    assert_eq!(queue.pop_front(), Some(C));
    assert_eq!(queue.pop_front(), None);
}

#[test]
fn full_queue_rejects_a_new_task_and_stays_intact() {
    let mut queue = queue_of::<2>(&[A, B]);
    assert_eq!(queue.push_back(C), Err(QueueError::Full));
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.pop_front(), Some(A));
    assert_eq!(queue.pop_front(), Some(B));
}

#[test]
fn a_task_cannot_be_queued_twice() {
    let mut queue = queue_of::<4>(&[A, B]);
    assert_eq!(queue.push_back(A), Err(QueueError::Duplicate));
    assert_eq!(queue.len(), 2);
}

#[test]
fn task_id_zero_is_rejected() {
    let mut queue = RunQueue::<4>::new();
    assert_eq!(queue.push_back(0), Err(QueueError::InvalidTask));
    assert!(queue.is_empty());
}

#[test]
fn contains_tracks_membership() {
    let mut queue = queue_of::<4>(&[A, B]);
    assert!(queue.contains(A));
    assert!(!queue.contains(C));
    queue.pop_front();
    assert!(!queue.contains(A));
}

#[test]
fn queue_wraps_around_its_storage() {
    let mut queue = RunQueue::<3>::new();
    for round in 0..10u64 {
        let task = 100 + round;
        queue.push_back(task).unwrap();
        queue.push_back(task + 1000).unwrap();
        assert_eq!(queue.pop_front(), Some(task));
        assert_eq!(queue.pop_front(), Some(task + 1000));
        assert!(queue.is_empty());
    }
}

// --- Décisions Round-Robin --------------------------------------------------------

#[test]
fn yield_puts_current_at_the_back_and_picks_the_head() {
    let mut queue = queue_of::<4>(&[B, C]);
    assert_eq!(run_queue::yield_next(&mut queue, A), Ok(B));
    assert_eq!(queue.pop_front(), Some(C));
    assert_eq!(queue.pop_front(), Some(A));
}

#[test]
fn yield_with_nobody_else_ready_resumes_the_same_task() {
    let mut queue = RunQueue::<4>::new();
    assert_eq!(run_queue::yield_next(&mut queue, A), Ok(A));
    assert!(queue.is_empty());
}

#[test]
fn yield_of_a_task_already_queued_is_refused() {
    let mut queue = queue_of::<4>(&[A, B]);
    assert_eq!(
        run_queue::yield_next(&mut queue, A),
        Err(QueueError::Duplicate)
    );
}

#[test]
fn exit_does_not_requeue_the_current_task() {
    let mut queue = queue_of::<4>(&[B, C]);
    assert_eq!(run_queue::exit_next(&mut queue), Some(B));
    assert_eq!(run_queue::exit_next(&mut queue), Some(C));
    assert_eq!(run_queue::exit_next(&mut queue), None);
}

#[test]
fn simulated_cooperative_run_rotates_strictly_then_drains() {
    // Trois tâches, quatre itérations chacune : yield après chaque itération
    // sauf la dernière, où la tâche sort.
    const ITERATIONS: u64 = 4;
    let mut queue = queue_of::<4>(&[B, C]);
    let mut current = Some(A);
    let mut runs = std::collections::HashMap::new();
    let mut trace = Vec::new();

    while let Some(task) = current {
        trace.push(task);
        let done = runs.entry(task).or_insert(0u64);
        *done += 1;
        current = if *done == ITERATIONS {
            run_queue::exit_next(&mut queue)
        } else {
            Some(run_queue::yield_next(&mut queue, task).unwrap())
        };
    }

    assert_eq!(trace, [A, B, C, A, B, C, A, B, C, A, B, C]);
    assert!(queue.is_empty());
    assert!(run_queue::is_round_robin(&trace, &[A, B, C], ITERATIONS));
}

// --- Contrôle de trace ------------------------------------------------------------

#[test]
fn strict_rotation_trace_is_accepted() {
    let trace = [A, B, C, A, B, C];
    assert!(run_queue::is_round_robin(&trace, &[A, B, C], 2));
}

#[test]
fn swapped_entries_are_rejected() {
    let trace = [A, B, C, B, A, C];
    assert!(!run_queue::is_round_robin(&trace, &[A, B, C], 2));
}

#[test]
fn missing_or_extra_runs_are_rejected() {
    assert!(!run_queue::is_round_robin(&[A, B, C, A, B], &[A, B, C], 2));
    assert!(!run_queue::is_round_robin(
        &[A, B, C, A, B, C, A],
        &[A, B, C],
        2
    ));
}

#[test]
fn a_task_running_twice_in_a_row_is_rejected() {
    let trace = [A, A, B, C, B, C];
    assert!(!run_queue::is_round_robin(&trace, &[A, B, C], 2));
}

#[test]
fn empty_task_set_or_zero_iterations_is_rejected() {
    assert!(!run_queue::is_round_robin(&[], &[], 2));
    assert!(!run_queue::is_round_robin(&[], &[A, B, C], 0));
}
