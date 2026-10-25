//! Tests hôte du rescheduling à distance (K004-08) : spinlock, retrait d'une
//! tâche d'une run queue, décisions d'IPI et de migration, contrôle du placement.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use elyon_core::percpu_sched::{self, Migration, Placed, PlacementError, TraceEntry, TraceError};
use elyon_core::run_queue::RunQueue;
use elyon_core::sync::RawSpinLock;

// --- Spinlock ---------------------------------------------------------------------

#[test]
fn lock_is_free_then_held_then_free() {
    let lock = RawSpinLock::new();
    assert!(!lock.is_locked());
    lock.lock();
    assert!(lock.is_locked());
    assert!(!lock.try_lock());
    lock.unlock();
    assert!(lock.try_lock());
    lock.unlock();
}

#[test]
fn lock_reports_contended_spins() {
    let lock = RawSpinLock::new();
    assert_eq!(lock.lock(), 0, "uncontended lock does not spin");
    lock.unlock();
}

#[test]
fn concurrent_read_modify_write_loses_no_update_under_the_lock() {
    // Le compteur est mis à jour par load puis store, non atomiquement : seule
    // l'exclusion mutuelle du verrou empêche les mises à jour perdues.
    const THREADS: usize = 8;
    const ROUNDS: usize = 20_000;
    let lock = Arc::new(RawSpinLock::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let lock = Arc::clone(&lock);
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..ROUNDS {
                    lock.lock();
                    let value = counter.load(Ordering::Relaxed);
                    std::hint::spin_loop();
                    counter.store(value + 1, Ordering::Relaxed);
                    lock.unlock();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(counter.load(Ordering::Relaxed), THREADS * ROUNDS);
    assert!(!lock.is_locked());
}

// --- Retrait d'une tâche d'une file -----------------------------------------------

#[test]
fn remove_takes_a_task_out_and_keeps_the_others_in_order() {
    let mut queue = RunQueue::<4>::new();
    for task in [3, 4, 5] {
        queue.push_back(task).unwrap();
    }
    assert!(queue.remove(4));
    assert!(!queue.contains(4));
    assert_eq!(queue.pop_front(), Some(3));
    assert_eq!(queue.pop_front(), Some(5));
    assert_eq!(queue.pop_front(), None);
}

#[test]
fn remove_of_an_absent_task_changes_nothing() {
    let mut queue = RunQueue::<4>::new();
    queue.push_back(3).unwrap();
    assert!(!queue.remove(9));
    assert_eq!(queue.len(), 1);
}

#[test]
fn remove_works_across_the_wrap_around() {
    let mut queue = RunQueue::<3>::new();
    for task in [1, 2, 3] {
        queue.push_back(task).unwrap();
    }
    queue.pop_front();
    queue.pop_front();
    queue.push_back(4).unwrap();
    queue.push_back(5).unwrap();
    assert!(queue.remove(4));
    assert_eq!(queue.pop_front(), Some(3));
    assert_eq!(queue.pop_front(), Some(5));
}

// --- Décisions --------------------------------------------------------------------

#[test]
fn ipi_is_sent_only_to_another_cpu() {
    assert!(percpu_sched::should_send_ipi(2, 0));
    assert!(!percpu_sched::should_send_ipi(1, 1));
}

#[test]
fn affinity_change_keeps_a_task_that_may_stay() {
    assert_eq!(
        percpu_sched::migrate(Some(1), 0b0110, 0b1111),
        Ok(Migration::Stay(1))
    );
}

#[test]
fn affinity_change_moves_a_task_whose_cpu_is_excluded() {
    assert_eq!(
        percpu_sched::migrate(Some(1), 0b0001, 0b1111),
        Ok(Migration::Move { from: 1, to: 0 })
    );
}

#[test]
fn affinity_change_of_an_unplaced_task_places_it() {
    assert_eq!(
        percpu_sched::migrate(None, 0b1000, 0b1111),
        Ok(Migration::Place(3))
    );
}

#[test]
fn affinity_change_to_an_impossible_mask_is_rejected() {
    assert_eq!(
        percpu_sched::migrate(Some(1), 0, 0b1111),
        Err(PlacementError::EmptyAffinity)
    );
    assert_eq!(
        percpu_sched::migrate(Some(1), 0b1000, 0b0111),
        Err(PlacementError::NoOnlineCpu)
    );
}

// --- Contrôle du placement --------------------------------------------------------

fn e(cpu: u32, task: u64) -> TraceEntry {
    TraceEntry { cpu, task }
}

const EXPECTED: [Placed; 3] = [
    Placed {
        task: 30,
        cpu: 1,
        runs: 1,
    },
    Placed {
        task: 31,
        cpu: 2,
        runs: 2,
    },
    Placed {
        task: 32,
        cpu: 0,
        runs: 1,
    },
];

#[test]
fn every_task_on_its_cpu_with_its_run_count_is_accepted() {
    let trace = [e(2, 31), e(1, 30), e(0, 32), e(2, 31)];
    assert_eq!(percpu_sched::check_placement(&trace, &EXPECTED), Ok(()));
}

#[test]
fn a_task_on_another_cpu_is_rejected() {
    let trace = [e(2, 31), e(3, 30), e(0, 32), e(2, 31)];
    assert_eq!(
        percpu_sched::check_placement(&trace, &EXPECTED),
        Err(TraceError::WrongCpu { task: 30, cpu: 3 })
    );
}

#[test]
fn a_wrong_run_count_is_rejected() {
    let trace = [e(2, 31), e(1, 30), e(0, 32)];
    assert_eq!(
        percpu_sched::check_placement(&trace, &EXPECTED),
        Err(TraceError::RunCount { task: 31 })
    );
}

#[test]
fn an_unknown_task_is_rejected() {
    let trace = [e(2, 31), e(1, 30), e(0, 32), e(2, 31), e(1, 99)];
    assert_eq!(
        percpu_sched::check_placement(&trace, &EXPECTED),
        Err(TraceError::UnknownTask { task: 99 })
    );
}
