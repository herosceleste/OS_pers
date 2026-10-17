#![no_std]

pub mod arch;
pub mod bootinfo;
pub mod serial;

use bootinfo::BootInfo;

pub fn kernel_main(info: &BootInfo) -> ! {
    arch::x86_64::cpu::disable_interrupts();

    kprintln!("[kernel] Elyon Kernel 0.0.1 entered");
    kprintln!(
        "[kernel] UEFI Boot Services exited: {}",
        info.boot_services_exited
    );

    let vendor = arch::x86_64::cpu::vendor_string();
    kprintln!("[cpu] vendor={}", vendor.as_str());
    kprintln!("[memory] regions={}", info.memory_region_count);
    kprintln!(
        "[memory] total={} MiB",
        info.total_memory_bytes / 1024 / 1024
    );
    kprintln!(
        "[memory] conventional={} MiB",
        info.usable_memory_bytes / 1024 / 1024
    );

    if info.memory_map_truncated {
        kprintln!(
            "[memory] warning: map truncated to {} regions",
            bootinfo::MAX_MEMORY_REGIONS
        );
    }

    arch::x86_64::gdt::init();
    kprintln!("[arch] GDT=OK");

    arch::x86_64::idt::init();
    kprintln!("[arch] IDT=OK exceptions=guarded");

    let cr0 = arch::x86_64::cpu::read_cr0();
    let cr3 = arch::x86_64::cpu::read_cr3();
    let cr4 = arch::x86_64::cpu::read_cr4();
    kprintln!(
        "[cpu] CR0={:#018x} CR3={:#018x} CR4={:#018x}",
        cr0,
        cr3,
        cr4
    );

    kprintln!("ELYON_KERNEL_READY");
    kprintln!("elyon> kernel bootstrap complete; scheduler not started in 0.0.1");

    arch::x86_64::cpu::halt_forever()
}

pub fn panic_report(info: &core::panic::PanicInfo<'_>) -> ! {
    kprintln!("[PANIC] {}", info);
    arch::x86_64::cpu::halt_forever()
}
