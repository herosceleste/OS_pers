//! Verrous irq-save du noyau et invariants de synchronisation (K004-10).
//!
//! `IrqSpinLock<T>` porte la donnée qu'il protège. Le prendre masque les IRQ
//! locales (un handler du même CPU ne peut donc jamais attendre un verrou tenu
//! par le code qu'il a interrompu), vérifie l'ordre des classes tenues par ce
//! CPU (`elyon_core::lock_order`) et le libérer restaure l'état d'IF. Avant
//! chaque changement de contexte, le scheduler vérifie qu'aucun verrou n'est
//! tenu. Toute violation arrête le noyau : elle signale un interblocage possible,
//! même si ce boot ne l'a pas produit.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU64, Ordering};

use elyon_core::lock_order::{self, HeldLocks, LockClass, LockOrderError, SwitchError};
use elyon_core::sync::RawSpinLock;

use crate::arch::x86_64::cpu;
use crate::arch::x86_64::cpu_local::{self, LOCK_CLASSES};
use crate::kprintln;

const _: () = assert!(LockClass::ALL.len() == LOCK_CLASSES);
static SWITCH_CHECKS: AtomicU64 = AtomicU64::new(0);

/// Compteur à écrivain unique (le CPU propriétaire de la zone) : pas besoin
/// d'instruction `lock`.
fn bump(counter: &AtomicU64, by: u64) {
    counter.store(counter.load(Ordering::Relaxed) + by, Ordering::Relaxed);
}

pub struct IrqSpinLock<T> {
    raw: RawSpinLock,
    class: LockClass,
    data: UnsafeCell<T>,
}

// SAFETY: `data` n'est accessible qu'à travers un `IrqGuard`, qui n'existe que
// verrou tenu ; un seul CPU y accède donc à la fois.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    pub const fn new(class: LockClass, data: T) -> Self {
        Self {
            raw: RawSpinLock::new(),
            class,
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> IrqGuard<'_, T> {
        let interrupts = cpu::interrupts_enabled();
        cpu::disable_interrupts();
        let local = cpu_local::current();
        let held = HeldLocks::from_bits(local.held_locks.load(Ordering::Relaxed));
        let next = match lock_order::check_acquire(held, self.class) {
            Ok(next) => next,
            Err(error) => order_violation(error),
        };
        // Seul le premier verrou pris retient l'état d'IF : il ne sera restauré
        // qu'au relâchement du dernier, quel que soit l'ordre des libérations.
        if held == HeldLocks::NONE {
            local.irq_before_locks.store(interrupts, Ordering::Relaxed);
        }
        local.held_locks.store(next.bits(), Ordering::Relaxed);
        let spins = self.raw.lock();
        let class = self.class as usize;
        bump(&local.lock_acquisitions[class], 1);
        bump(&local.lock_contention[class], spins);
        IrqGuard {
            lock: self,
            _not_send: PhantomData,
        }
    }
}

/// Garde de verrou. `!Send` : relâché sur le CPU qui l'a pris, dont il met à
/// jour l'état ; il ne traverse jamais un switch (`assert_switch_allowed`).
pub struct IrqGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    _not_send: PhantomData<*mut ()>,
}

impl<T> Deref for IrqGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: le verrou est tenu pendant toute la vie du garde.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for IrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: le verrou tenu garantit l'unicité de la référence mutable.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.raw.unlock();
        let local = cpu_local::current();
        let held = HeldLocks::from_bits(local.held_locks.load(Ordering::Relaxed));
        let next = match lock_order::release(held, self.lock.class) {
            Ok(next) => next,
            Err(error) => order_violation(error),
        };
        local.held_locks.store(next.bits(), Ordering::Relaxed);
        if next == HeldLocks::NONE && local.irq_before_locks.load(Ordering::Relaxed) {
            cpu::enable_interrupts();
        }
    }
}

/// Appelé juste avant chaque changement de contexte.
pub fn assert_switch_allowed() {
    SWITCH_CHECKS.fetch_add(1, Ordering::Relaxed);
    let held = HeldLocks::from_bits(cpu_local::current().held_locks.load(Ordering::Relaxed));
    if let Err(error) = lock_order::check_switch(held, cpu::interrupts_enabled()) {
        switch_violation(error);
    }
}

fn order_violation(error: LockOrderError) -> ! {
    kprintln!("[lock] FAILED cpu={} order={:?}", cpu_local::index(), error);
    cpu::halt_forever()
}

fn switch_violation(error: SwitchError) -> ! {
    kprintln!(
        "[lock] FAILED cpu={} switch={:?}",
        cpu_local::index(),
        error
    );
    cpu::halt_forever()
}

pub fn acquisitions(class: LockClass) -> u64 {
    cpu_local::sum(|area| &area.lock_acquisitions[class as usize])
}

pub fn contention(class: LockClass) -> u64 {
    cpu_local::sum(|area| &area.lock_contention[class as usize])
}

pub fn total_contention() -> u64 {
    LockClass::ALL.iter().map(|&class| contention(class)).sum()
}

pub fn switch_checks() -> u64 {
    SWITCH_CHECKS.load(Ordering::Relaxed)
}
