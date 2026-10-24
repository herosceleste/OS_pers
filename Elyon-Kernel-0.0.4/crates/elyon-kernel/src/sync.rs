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
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use elyon_core::lock_order::{self, HeldLocks, LockClass, LockOrderError, SwitchError};
use elyon_core::sync::RawSpinLock;

use crate::arch::x86_64::{apic, cpu, percpu};
use crate::kprintln;
use crate::sched::MAX_SCHED_CPUS;

const CLASSES: usize = LockClass::ALL.len();

/// Classes tenues, par CPU logique.
static HELD: [AtomicU32; MAX_SCHED_CPUS] = [const { AtomicU32::new(0) }; MAX_SCHED_CPUS];
static ACQUISITIONS: [AtomicU64; CLASSES] = [const { AtomicU64::new(0) }; CLASSES];
static CONTENTION: [AtomicU64; CLASSES] = [const { AtomicU64::new(0) }; CLASSES];
static SWITCH_CHECKS: AtomicU64 = AtomicU64::new(0);

/// CPU logique courant. Avant l'initialisation du LAPIC et des slots per-CPU,
/// seul le BSP s'exécute : son slot est 0.
fn cpu_slot() -> usize {
    apic::current_id()
        .ok()
        .and_then(percpu::index_of_apic)
        .filter(|&cpu| cpu < MAX_SCHED_CPUS)
        .unwrap_or(0)
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
        let slot = cpu_slot();
        let held = HeldLocks::from_bits(HELD[slot].load(Ordering::Relaxed));
        match lock_order::check_acquire(held, self.class) {
            Ok(next) => HELD[slot].store(next.bits(), Ordering::Relaxed),
            Err(error) => order_violation(error),
        }
        let spins = self.raw.lock();
        let class = self.class as usize;
        ACQUISITIONS[class].fetch_add(1, Ordering::Relaxed);
        CONTENTION[class].fetch_add(spins, Ordering::Relaxed);
        IrqGuard {
            lock: self,
            slot,
            interrupts,
        }
    }
}

pub struct IrqGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    slot: usize,
    interrupts: bool,
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
        let held = HeldLocks::from_bits(HELD[self.slot].load(Ordering::Relaxed));
        match lock_order::release(held, self.lock.class) {
            Ok(next) => HELD[self.slot].store(next.bits(), Ordering::Relaxed),
            Err(error) => order_violation(error),
        }
        if self.interrupts {
            cpu::enable_interrupts();
        }
    }
}

/// Appelé juste avant chaque changement de contexte.
pub fn assert_switch_allowed() {
    SWITCH_CHECKS.fetch_add(1, Ordering::Relaxed);
    let held = HeldLocks::from_bits(HELD[cpu_slot()].load(Ordering::Relaxed));
    if let Err(error) = lock_order::check_switch(held, cpu::interrupts_enabled()) {
        switch_violation(error);
    }
}

fn order_violation(error: LockOrderError) -> ! {
    kprintln!("[lock] FAILED cpu={} order={:?}", cpu_slot(), error);
    cpu::halt_forever()
}

fn switch_violation(error: SwitchError) -> ! {
    kprintln!("[lock] FAILED cpu={} switch={:?}", cpu_slot(), error);
    cpu::halt_forever()
}

pub fn acquisitions(class: LockClass) -> u64 {
    ACQUISITIONS[class as usize].load(Ordering::Relaxed)
}

pub fn contention(class: LockClass) -> u64 {
    CONTENTION[class as usize].load(Ordering::Relaxed)
}

pub fn total_contention() -> u64 {
    CONTENTION
        .iter()
        .map(|counter| counter.load(Ordering::Relaxed))
        .sum()
}

pub fn switch_checks() -> u64 {
    SWITCH_CHECKS.load(Ordering::Relaxed)
}
