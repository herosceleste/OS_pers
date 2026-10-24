//! Zone locale à chaque CPU, adressée par GS (K004-10).
//!
//! `IA32_GS_BASE` pointe vers la zone du CPU courant ; son premier champ est
//! l'index logique, lu par `mov gs:[0]` en un ou deux cycles au lieu d'une
//! lecture MMIO de l'APIC ID suivie d'une recherche. Le BSP la règle en tête de
//! `_start`, chaque AP en tête de son point d'entrée, avant toute prise de
//! verrou. Une zone par CPU, alignée sur une ligne de cache : les compteurs d'un
//! CPU ne partagent pas de ligne avec ceux d'un autre.
//!
//! Le noyau ne s'exécute pas encore en ring 3 : il n'y a pas de `swapgs`. Le
//! ring 3 (0.0.5) devra échanger GS à chaque entrée et sortie du noyau.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::{MAX_CPU_SLOTS, cpu};

const IA32_GS_BASE: u32 = 0xC000_0101;
/// Classes de verrous suivies (`elyon_core::lock_order::LockClass::ALL`).
pub const LOCK_CLASSES: usize = 4;

#[repr(C, align(64))]
pub struct CpuLocal {
    /// Index logique du CPU ; doit rester le premier champ (`gs:[0]`).
    index: u64,
    /// Classes de verrous tenues par ce CPU (`HeldLocks::bits`).
    pub held_locks: AtomicU32,
    /// État d'IF au moment où le premier verrou a été pris, restauré quand le
    /// dernier est relâché.
    pub irq_before_locks: AtomicBool,
    pub lock_acquisitions: [AtomicU64; LOCK_CLASSES],
    pub lock_contention: [AtomicU64; LOCK_CLASSES],
    /// Contexte quitté par le dernier switch de ce CPU, dont l'indicateur
    /// `on_cpu` reste à effacer par le contexte entrant.
    pub switched_from: AtomicU64,
}

impl CpuLocal {
    const fn new(index: usize) -> Self {
        Self {
            index: index as u64,
            held_locks: AtomicU32::new(0),
            irq_before_locks: AtomicBool::new(false),
            lock_acquisitions: [const { AtomicU64::new(0) }; LOCK_CLASSES],
            lock_contention: [const { AtomicU64::new(0) }; LOCK_CLASSES],
            switched_from: AtomicU64::new(0),
        }
    }
}

static AREAS: [CpuLocal; MAX_CPU_SLOTS] = {
    let mut areas = [const { CpuLocal::new(0) }; MAX_CPU_SLOTS];
    let mut index = 0;
    while index < MAX_CPU_SLOTS {
        areas[index] = CpuLocal::new(index);
        index += 1;
    }
    areas
};

/// Fait pointer GS vers la zone du CPU `index`. À appeler une fois par CPU,
/// IRQ masquées, avant toute prise de verrou.
pub fn init(index: usize) {
    if index >= MAX_CPU_SLOTS {
        cpu::halt_forever();
    }
    let base = &AREAS[index] as *const CpuLocal as u64;
    // SAFETY: IA32_GS_BASE existe sur tout x86_64 ; la zone est statique, donc
    // valide pour toute la vie du noyau.
    unsafe { cpu::write_msr(IA32_GS_BASE, base) };
    if self::index() != index {
        cpu::halt_forever();
    }
}

/// Index logique du CPU courant.
#[inline]
pub fn index() -> usize {
    let index: u64;
    // SAFETY: GS pointe vers une zone `CpuLocal` réglée par `init` avant tout
    // appel ; seul le premier champ est lu.
    unsafe {
        asm!("mov {}, gs:[0]", out(reg) index, options(nostack, preserves_flags, readonly));
    }
    index as usize
}

/// Zone du CPU courant.
#[inline]
pub fn current() -> &'static CpuLocal {
    &AREAS[index()]
}

/// Somme sur tous les CPU d'un compteur de verrous.
pub fn sum(counter: impl Fn(&CpuLocal) -> &AtomicU64) -> u64 {
    AREAS
        .iter()
        .map(|area| counter(area).load(Ordering::Relaxed))
        .sum()
}
