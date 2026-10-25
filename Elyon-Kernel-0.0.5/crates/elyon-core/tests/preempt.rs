//! Tests hôte de la préemption par timer (K004-04) : quantum, décision par tick
//! et contrôle de la trace des tranches.

use elyon_core::preempt::{self, PreemptError, Quantum, SliceClock, TickDecision};

const A: u64 = 9;
const B: u64 = 10;
const C: u64 = 11;

// --- Quantum ----------------------------------------------------------------------

#[test]
fn zero_quantum_is_rejected() {
    assert_eq!(Quantum::new(0), Err(PreemptError::ZeroQuantum));
}

#[test]
fn quantum_keeps_its_tick_count() {
    assert_eq!(Quantum::new(3).unwrap().ticks(), 3);
}

// --- Décision par tick ------------------------------------------------------------

#[test]
fn one_tick_quantum_preempts_on_every_tick() {
    let mut clock = SliceClock::new(Quantum::new(1).unwrap());
    assert_eq!(clock.on_tick(), TickDecision::Preempt);
    clock.reset();
    assert_eq!(clock.on_tick(), TickDecision::Preempt);
}

#[test]
fn multi_tick_quantum_continues_until_exhausted() {
    let mut clock = SliceClock::new(Quantum::new(3).unwrap());
    assert_eq!(clock.on_tick(), TickDecision::Continue);
    assert_eq!(clock.remaining(), 2);
    assert_eq!(clock.on_tick(), TickDecision::Continue);
    assert_eq!(clock.on_tick(), TickDecision::Preempt);
    assert_eq!(clock.remaining(), 0);
}

#[test]
fn exhausted_slice_keeps_asking_for_preemption_until_reset() {
    // Si la file est vide, le scheduler garde la tâche : tant qu'il ne remet pas
    // la tranche à zéro, chaque tick redemande la préemption.
    let mut clock = SliceClock::new(Quantum::new(2).unwrap());
    clock.on_tick();
    assert_eq!(clock.on_tick(), TickDecision::Preempt);
    assert_eq!(clock.on_tick(), TickDecision::Preempt);
    clock.reset();
    assert_eq!(clock.remaining(), 2);
    assert_eq!(clock.on_tick(), TickDecision::Continue);
}

#[test]
fn reset_gives_a_full_slice_back() {
    let mut clock = SliceClock::new(Quantum::new(4).unwrap());
    clock.on_tick();
    clock.on_tick();
    clock.reset();
    assert_eq!(clock.remaining(), 4);
}

#[test]
fn simulated_ticks_produce_one_preemption_per_quantum() {
    let mut clock = SliceClock::new(Quantum::new(2).unwrap());
    let mut preemptions = 0;
    for _ in 0..10 {
        if clock.on_tick() == TickDecision::Preempt {
            preemptions += 1;
            clock.reset();
        }
    }
    assert_eq!(preemptions, 5);
}

// --- Trace des tranches -----------------------------------------------------------

#[test]
fn strict_rotation_of_any_length_is_accepted() {
    assert!(preempt::is_rotation(&[A, B, C, A, B], &[A, B, C]));
    assert!(preempt::is_rotation(
        &[A, B, C, A, B, C, A, B, C, A],
        &[A, B, C]
    ));
}

#[test]
fn rotation_must_start_with_the_first_task() {
    assert!(!preempt::is_rotation(&[B, C, A], &[A, B, C]));
}

#[test]
fn a_repeated_or_skipped_task_breaks_the_rotation() {
    assert!(!preempt::is_rotation(&[A, A, B, C], &[A, B, C]));
    assert!(!preempt::is_rotation(&[A, C, B], &[A, B, C]));
}

#[test]
fn empty_trace_or_task_set_is_not_a_rotation() {
    assert!(!preempt::is_rotation(&[], &[A, B, C]));
    assert!(!preempt::is_rotation(&[A], &[]));
}

#[test]
fn min_slices_counts_the_least_served_task() {
    let trace = [A, B, C, A, B, C, A];
    assert_eq!(preempt::min_slices(&trace, &[A, B, C]), 2);
}

#[test]
fn a_task_that_never_ran_has_zero_slices() {
    assert_eq!(preempt::min_slices(&[A, B, A, B], &[A, B, C]), 0);
}

#[test]
fn min_slices_of_an_empty_task_set_is_zero() {
    assert_eq!(preempt::min_slices(&[A, B], &[]), 0);
}
