//! Primitives de synchronisation sans donnée ni `unsafe` (K004-08).
//!
//! Le noyau associe le verrou aux données qu'il protège (files per-CPU) ; ce
//! module ne fournit que l'exclusion mutuelle, testée sur l'hôte avec de vrais
//! threads.

use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};

/// Spinlock test-and-test-and-set. L'appelant masque les IRQ locales avant de
/// le prendre si un handler d'interruption du même CPU peut aussi le prendre.
pub struct RawSpinLock {
    locked: AtomicBool,
}

impl RawSpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Prend le verrou et retourne le nombre de tours d'attente (contention).
    pub fn lock(&self) -> u64 {
        let mut spins = 0u64;
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return spins;
            }
            while self.locked.load(Ordering::Relaxed) {
                spins += 1;
                spin_loop();
            }
        }
    }

    pub fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

impl Default for RawSpinLock {
    fn default() -> Self {
        Self::new()
    }
}
