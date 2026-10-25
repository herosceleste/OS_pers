pub const MAX_CPU_SLOTS: usize = 64;

pub mod apic;
pub mod cpu;
pub mod cpu_local;
pub mod gdt;
pub mod idt;
pub mod ioapic;
pub mod paging;
pub mod percpu;
pub mod smp;
