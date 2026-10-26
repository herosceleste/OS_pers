//! K005-05 : TLB shootdown.
//!
//! Retirer une traduction (alias identité d'une page utilisateur, mappage
//! utilisateur à la libération) ne suffit pas sur le seul CPU qui modifie les
//! tables : les autres CPU peuvent la garder dans leur TLB. `shootdown`
//! l'invalide localement, publie l'adresse sous une nouvelle génération, envoie
//! l'IPI `TLB_VECTOR` à chaque autre CPU en ligne et attend que chacun ait
//! acquitté cette génération après son propre `invlpg`. Une seule invalidation
//! est en vol à la fois ; un CPU qui attend son tour sert entre-temps celle qui
//! le vise : deux émetteurs IRQ masquées ne peuvent pas s'interbloquer.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use elyon_core::user_pages;

use crate::arch::x86_64::{apic, cpu_local, percpu};
use crate::sched;

pub const TLB_VECTOR: u8 = 0xF3;
/// Tours de `pause` au plus en attente des acquittements (plusieurs secondes).
const ACK_SPINS: u64 = 500_000_000;

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static ADDRESS: AtomicU64 = AtomicU64::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Dernière génération invalidée par chaque CPU.
static ACKS: [AtomicU64; sched::MAX_SCHED_CPUS] =
    [const { AtomicU64::new(0) }; sched::MAX_SCHED_CPUS];
/// IPI de shootdown reçues, par CPU.
static RECEIVED: [AtomicU64; sched::MAX_SCHED_CPUS] =
    [const { AtomicU64::new(0) }; sched::MAX_SCHED_CPUS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlbError {
    /// L'APIC a refusé l'IPI.
    Ipi,
    /// Un CPU n'a pas acquitté dans le délai : sa TLB peut garder la traduction.
    Timeout,
}

impl TlbError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ipi => "local APIC refused the TLB shootdown IPI",
            Self::Timeout => "a CPU did not acknowledge the TLB shootdown",
        }
    }
}

pub fn invalidate_local(address: u64) {
    // SAFETY: `invlpg` n'invalide qu'une traduction du TLB de ce CPU.
    unsafe { asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags)) };
}

/// Sert l'invalidation en vol si ce CPU ne l'a pas encore acquittée.
fn service(cpu: usize) {
    let generation = GENERATION.load(Ordering::Acquire);
    if cpu < sched::MAX_SCHED_CPUS && ACKS[cpu].load(Ordering::Acquire) < generation {
        invalidate_local(ADDRESS.load(Ordering::Acquire));
        ACKS[cpu].store(generation, Ordering::Release);
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_tlb_ipi_dispatch() {
    let cpu = cpu_local::index();
    if cpu < sched::MAX_SCHED_CPUS {
        RECEIVED[cpu].fetch_add(1, Ordering::AcqRel);
    }
    service(cpu);
    let _ = apic::eoi();
}

fn online_mask() -> u64 {
    (0..percpu::cpu_count().min(sched::MAX_SCHED_CPUS))
        .filter(|&index| percpu::is_online(index))
        .fold(0u64, |mask, index| mask | (1 << index))
}

/// Invalide `address` sur tous les CPU en ligne. Un échec laisse une
/// traduction possible ailleurs : l'appelant doit s'arrêter fail-closed.
pub fn shootdown(address: u64) -> Result<(), TlbError> {
    let cpu = cpu_local::index();
    invalidate_local(address);
    while IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        service(cpu);
        core::hint::spin_loop();
    }
    let result = broadcast(cpu, address);
    IN_FLIGHT.store(false, Ordering::Release);
    result
}

fn broadcast(cpu: usize, address: u64) -> Result<(), TlbError> {
    let targets = online_mask() & user_pages::shootdown_targets(sched::MAX_SCHED_CPUS, cpu);
    if targets == 0 {
        return Ok(());
    }
    // L'adresse est publiée avant la génération qui la désigne : un CPU qui
    // voit la nouvelle génération lit cette adresse.
    ADDRESS.store(address, Ordering::Release);
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    for target in (0..sched::MAX_SCHED_CPUS).filter(|&target| targets & (1 << target) != 0) {
        let apic_id = percpu::snapshot(target).ok_or(TlbError::Ipi)?.apic_id;
        apic::send_fixed(apic_id, TLB_VECTOR).map_err(|_| TlbError::Ipi)?;
    }
    for _ in 0..ACK_SPINS {
        let acks: [u64; sched::MAX_SCHED_CPUS] =
            core::array::from_fn(|index| ACKS[index].load(Ordering::Acquire));
        if user_pages::all_acked(&acks, targets, generation) {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(TlbError::Timeout)
}

/// IPI de shootdown reçues par `cpu` depuis le boot.
pub fn received(cpu: usize) -> u64 {
    RECEIVED[cpu].load(Ordering::Acquire)
}
