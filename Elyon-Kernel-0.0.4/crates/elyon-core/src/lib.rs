//! Logique pure d'Elyon, sans accès matériel : testable sur l'hôte avec `cargo test`.
//!
//! Le noyau fournit les accès physiques, MMIO et ports ; cette crate ne fait
//! que décoder, valider et planifier.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod acpi;
pub mod cpu_context;
pub mod fpu;
pub mod ioapic;
pub mod percpu_sched;
pub mod pit;
pub mod preempt;
pub mod run_queue;
pub mod sync;
pub mod task_stack;
pub mod timer_queue;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_on_host() {
        assert_eq!(core::mem::size_of::<u8>(), 1);
    }
}
