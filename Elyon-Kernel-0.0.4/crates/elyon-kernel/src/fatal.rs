//! Arrêt fail-closed de tout le système (K004-10).
//!
//! Une erreur fatale détectée sur un CPU arrêtait ce seul CPU : les autres
//! continuaient et pouvaient attendre sans fin un verrou qu'il tenait. Le CPU
//! qui détecte l'erreur envoie maintenant un NMI à tous les autres, qui
//! s'arrêtent sans rien imprimer ; il attend brièvement leur accusé puis
//! s'arrête à son tour. Un NMI n'est pas masqué par IF : même un CPU qui tourne
//! sur un spinlock IRQ masquées s'arrête.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::{apic, cpu, percpu};
use crate::kprintln;

static HALTING: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicU64 = AtomicU64::new(0);
/// Attente maximale des accusés d'arrêt, en tours de boucle.
const STOP_WAIT_SPINS: u64 = 50_000_000;

/// Arrête tous les CPU. Le diagnostic a déjà été imprimé par l'appelant.
pub fn halt_system() -> ! {
    cpu::disable_interrupts();
    if HALTING.swap(true, Ordering::AcqRel) {
        // Un autre CPU a déjà lancé l'arrêt ; son NMI arrive.
        cpu::halt_forever();
    }
    let online = (0..percpu::cpu_count())
        .filter(|&index| percpu::is_online(index))
        .count() as u64;
    let others = online.saturating_sub(1);
    if others > 0 {
        match apic::send_nmi_to_others() {
            Ok(()) => {
                let mut spins = 0;
                while STOPPED.load(Ordering::Acquire) < others && spins < STOP_WAIT_SPINS {
                    core::hint::spin_loop();
                    spins += 1;
                }
            }
            Err(_) => kprintln!("[halt] FAILED nmi broadcast"),
        }
    }
    kprintln!(
        "[halt] system stopped others={} stopped={}",
        others,
        STOPPED.load(Ordering::Acquire)
    );
    cpu::halt_forever()
}

/// Appelé par le dispatcher d'exceptions sur un NMI. Vrai si le NMI vient d'un
/// arrêt système ; le CPU ne revient alors jamais.
pub fn on_nmi() -> bool {
    if !HALTING.load(Ordering::Acquire) {
        return false;
    }
    STOPPED.fetch_add(1, Ordering::AcqRel);
    cpu::halt_forever()
}
