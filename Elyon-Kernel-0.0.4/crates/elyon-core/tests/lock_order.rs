//! Tests hôte de l'ordre des verrous (K004-10) : rangs, réentrance, libération,
//! condition de changement de contexte, et vivacité sous de vrais threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use elyon_core::lock_order::{self, HeldLocks, LockClass, LockOrderError, SwitchError};
use elyon_core::sync::RawSpinLock;

// --- Prise et libération ----------------------------------------------------------

#[test]
fn locks_taken_in_rank_order_are_accepted() {
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::Pmm).unwrap();
    let held = lock_order::check_acquire(held, LockClass::RunQueue).unwrap();
    let held = lock_order::check_acquire(held, LockClass::TimerQueue).unwrap();
    assert!(held.contains(LockClass::Pmm));
    assert!(held.contains(LockClass::RunQueue));
    assert!(held.contains(LockClass::TimerQueue));
}

#[test]
fn skipping_a_rank_is_allowed() {
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::RunQueue).unwrap();
    assert!(lock_order::check_acquire(held, LockClass::TimerQueue).is_ok());
}

#[test]
fn taking_a_lower_rank_while_holding_a_higher_one_is_a_violation() {
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::TimerQueue).unwrap();
    assert_eq!(
        lock_order::check_acquire(held, LockClass::RunQueue),
        Err(LockOrderError::Inversion {
            held: LockClass::TimerQueue,
            acquiring: LockClass::RunQueue,
        })
    );
}

#[test]
fn taking_a_held_class_again_is_reentrance() {
    // Deux files de CPU ne sont jamais tenues ensemble (migration en deux temps).
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::RunQueue).unwrap();
    assert_eq!(
        lock_order::check_acquire(held, LockClass::RunQueue),
        Err(LockOrderError::Reentrant(LockClass::RunQueue))
    );
}

#[test]
fn release_clears_the_class_and_rejects_an_unheld_one() {
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::RunQueue).unwrap();
    let held = lock_order::check_acquire(held, LockClass::TimerQueue).unwrap();
    let held = lock_order::release(held, LockClass::TimerQueue).unwrap();
    assert!(!held.contains(LockClass::TimerQueue));
    assert!(held.contains(LockClass::RunQueue));
    assert_eq!(
        lock_order::release(held, LockClass::Pmm),
        Err(LockOrderError::NotHeld(LockClass::Pmm))
    );
    let held = lock_order::release(held, LockClass::RunQueue).unwrap();
    assert_eq!(held, HeldLocks::NONE);
}

#[test]
fn bits_round_trip_for_per_cpu_storage() {
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::TimerQueue).unwrap();
    assert_eq!(HeldLocks::from_bits(held.bits()), held);
    assert_eq!(HeldLocks::NONE.bits(), 0);
}

// --- Changement de contexte -------------------------------------------------------

#[test]
fn a_switch_needs_no_lock_held_and_interrupts_masked() {
    assert_eq!(lock_order::check_switch(HeldLocks::NONE, false), Ok(()));
    assert_eq!(
        lock_order::check_switch(HeldLocks::NONE, true),
        Err(SwitchError::InterruptsEnabled)
    );
    let held = lock_order::check_acquire(HeldLocks::NONE, LockClass::RunQueue).unwrap();
    assert_eq!(
        lock_order::check_switch(held, false),
        Err(SwitchError::LockHeld(LockClass::RunQueue))
    );
}

// --- Vivacité sous de vrais threads -------------------------------------------------

#[test]
fn threads_taking_lock_subsets_in_rank_order_never_deadlock() {
    // Chaque thread prend, à chaque tour, un sous-ensemble des trois verrous, en
    // ordre de rang, vérifié par check_acquire ; il incrémente non atomiquement le
    // compteur de chaque verrou tenu. Tous terminent et aucun compte n'est perdu.
    const THREADS: u64 = 4;
    const ROUNDS: u64 = 20_000;
    const CLASSES: [LockClass; 3] = [LockClass::Pmm, LockClass::RunQueue, LockClass::TimerQueue];
    let locks: Arc<[RawSpinLock; 3]> = Arc::new([const { RawSpinLock::new() }; 3]);
    let counters: Arc<[AtomicU64; 3]> = Arc::new([const { AtomicU64::new(0) }; 3]);
    let expected: Vec<u64> = (0..3)
        .map(|class| {
            (0..THREADS)
                .map(|thread| {
                    (0..ROUNDS)
                        .filter(|round| (round + thread) % 7 & (1 << class) != 0)
                        .count() as u64
                })
                .sum()
        })
        .collect();
    let handles: Vec<_> = (0..THREADS)
        .map(|thread| {
            let locks = Arc::clone(&locks);
            let counters = Arc::clone(&counters);
            thread::spawn(move || {
                for round in 0..ROUNDS {
                    let subset = (round + thread) % 7;
                    let mut held = HeldLocks::NONE;
                    for (index, class) in CLASSES.iter().enumerate() {
                        if subset & (1 << index) != 0 {
                            held = lock_order::check_acquire(held, *class).unwrap();
                            locks[index].lock();
                        }
                    }
                    for index in 0..3 {
                        if subset & (1 << index) != 0 {
                            let value = counters[index].load(Ordering::Relaxed);
                            counters[index].store(value + 1, Ordering::Relaxed);
                        }
                    }
                    for (index, class) in CLASSES.iter().enumerate().rev() {
                        if subset & (1 << index) != 0 {
                            locks[index].unlock();
                            held = lock_order::release(held, *class).unwrap();
                        }
                    }
                    assert_eq!(held, HeldLocks::NONE);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    for class in 0..3 {
        assert_eq!(counters[class].load(Ordering::Relaxed), expected[class]);
    }
}
