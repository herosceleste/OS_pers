//! Tests hôte de la file temporelle (K004-09) : échéances, ordre de réveil,
//! annulation, calcul d'échéance et contrôle des traces de sommeil.

use elyon_core::timer_queue::{self, SleepError, SleepRecord, TimerQueue, TimerQueueError};

// --- File temporelle --------------------------------------------------------------

#[test]
fn expired_tasks_come_out_in_deadline_order() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert(7, 30).unwrap();
    queue.insert(8, 10).unwrap();
    queue.insert(9, 20).unwrap();
    assert_eq!(queue.next_deadline(), Some(10));
    assert_eq!(queue.pop_expired(30), Some(8));
    assert_eq!(queue.pop_expired(30), Some(9));
    assert_eq!(queue.pop_expired(30), Some(7));
    assert_eq!(queue.pop_expired(30), None);
    assert!(queue.is_empty());
}

#[test]
fn nothing_expires_before_its_deadline() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert(7, 10).unwrap();
    assert_eq!(queue.pop_expired(9), None);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.pop_expired(10), Some(7));
}

#[test]
fn equal_deadlines_keep_arrival_order() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert(5, 10).unwrap();
    queue.insert(3, 10).unwrap();
    queue.insert(4, 10).unwrap();
    assert_eq!(queue.pop_expired(10), Some(5));
    assert_eq!(queue.pop_expired(10), Some(3));
    assert_eq!(queue.pop_expired(10), Some(4));
}

#[test]
fn cancel_removes_only_that_task() {
    let mut queue = TimerQueue::<4>::new();
    queue.insert(7, 10).unwrap();
    queue.insert(8, 20).unwrap();
    assert!(queue.cancel(7));
    assert!(!queue.cancel(7));
    assert!(!queue.contains(7));
    assert_eq!(queue.pop_expired(100), Some(8));
    assert_eq!(queue.pop_expired(100), None);
}

#[test]
fn invalid_inserts_are_rejected() {
    let mut queue = TimerQueue::<2>::new();
    assert_eq!(queue.insert(0, 10), Err(TimerQueueError::InvalidTask));
    queue.insert(7, 10).unwrap();
    assert_eq!(queue.insert(7, 20), Err(TimerQueueError::Duplicate));
    queue.insert(8, 20).unwrap();
    assert_eq!(queue.insert(9, 30), Err(TimerQueueError::Full));
}

// --- Échéances ----------------------------------------------------------------------

#[test]
fn deadline_rounds_up_and_adds_the_partial_current_tick() {
    // 100 Hz : 1 tick = 10 ms. Le tick courant est déjà entamé, d'où +1.
    assert_eq!(timer_queue::deadline_for(50, 10, 100), 52);
    assert_eq!(timer_queue::deadline_for(50, 11, 100), 53);
    assert_eq!(timer_queue::deadline_for(50, 1, 100), 52);
    assert_eq!(timer_queue::deadline_for(50, 30, 100), 54);
}

#[test]
fn zero_duration_expires_immediately() {
    assert_eq!(timer_queue::deadline_for(50, 0, 100), 50);
}

#[test]
fn deadline_saturates_instead_of_overflowing() {
    assert_eq!(timer_queue::deadline_for(u64::MAX - 1, 10, 100), u64::MAX);
    assert_eq!(timer_queue::deadline_for(0, u64::MAX, 1000), u64::MAX);
}

// --- Contrôle des traces ------------------------------------------------------------

fn timed(task: u64, deadline: u64, woke_at: u64) -> SleepRecord {
    SleepRecord {
        task,
        deadline,
        woke_at,
        wakes: 1,
        explicit: false,
    }
}

#[test]
fn on_time_wakes_in_deadline_order_are_accepted() {
    let records = [timed(1, 14, 14), timed(2, 12, 13), timed(3, 13, 13)];
    assert_eq!(timer_queue::check_sleep(&records, 2), Ok(()));
}

#[test]
fn an_early_wake_is_rejected() {
    let records = [timed(1, 14, 13)];
    assert_eq!(
        timer_queue::check_sleep(&records, 2),
        Err(SleepError::Early { task: 1 })
    );
}

#[test]
fn a_late_wake_is_rejected() {
    let records = [timed(1, 14, 17)];
    assert_eq!(
        timer_queue::check_sleep(&records, 2),
        Err(SleepError::Late { task: 1 })
    );
}

#[test]
fn wakes_out_of_deadline_order_are_rejected() {
    let records = [timed(1, 12, 15), timed(2, 13, 14)];
    assert_eq!(
        timer_queue::check_sleep(&records, 3),
        Err(SleepError::OrderBroken { task: 2 })
    );
}

#[test]
fn a_double_or_missing_wake_is_rejected() {
    let mut record = timed(1, 12, 12);
    record.wakes = 2;
    assert_eq!(
        timer_queue::check_sleep(&[record], 2),
        Err(SleepError::WakeCount { task: 1 })
    );
    record.wakes = 0;
    assert_eq!(
        timer_queue::check_sleep(&[record], 2),
        Err(SleepError::WakeCount { task: 1 })
    );
}

#[test]
fn an_explicit_wake_must_come_before_the_deadline() {
    let mut record = timed(1, 40, 15);
    record.explicit = true;
    assert_eq!(timer_queue::check_sleep(&[record], 2), Ok(()));
    record.woke_at = 40;
    assert_eq!(
        timer_queue::check_sleep(&[record], 2),
        Err(SleepError::ExplicitNotEarly { task: 1 })
    );
}
