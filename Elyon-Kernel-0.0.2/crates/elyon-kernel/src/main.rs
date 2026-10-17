#![no_main]
#![no_std]

mod arch;
mod framebuffer;
mod memory;
mod serial;

use core::panic::PanicInfo;

use elyon_abi::{BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo};

use crate::arch::x86_64::{cpu, gdt, idt, paging};

const BUILD_ID: &str = match option_env!("ELYON_BUILD_ID") {
    Some(value) => value,
    None => "development",
};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text._start")]
pub extern "sysv64" fn _start(info_ptr: *const BootInfo) -> ! {
    serial::init();
    cpu::disable_interrupts();

    kprintln!("ELYON KERNEL 0.0.2");
    kprintln!("[kernel] build_id={}", BUILD_ID);

    if info_ptr.is_null() {
        kprintln!("[kernel] invalid BootInfo pointer");
        cpu::halt_forever();
    }

    let info = unsafe { &*info_ptr };
    validate_boot_info(info);

    kprintln!(
        "[kernel] handoff=OK regions={} conventional={} MiB",
        info.memory_region_count,
        info.usable_memory_bytes / 1024 / 1024
    );
    kprintln!(
        "[kernel] image={:#x}..{:#x} stack={:#x}..{:#x}",
        info.kernel_phys_start,
        info.kernel_phys_end,
        info.kernel_stack_start,
        info.kernel_stack_end
    );

    let vendor = cpu::vendor_string();
    kprintln!("[cpu] vendor={}", vendor.as_str());

    let code_selector = gdt::init();
    kprintln!("[arch] GDT=OK TSS=OK IST(#DF)=OK selector={:#x}", code_selector);

    idt::init(code_selector);
    kprintln!("[arch] IDT=OK exceptions=diagnostic");

    let pmm = memory::init(info);
    kprintln!(
        "[memory] PMM=OK managed={} MiB frames={} free={}",
        pmm.managed_mib,
        pmm.managed_frames,
        pmm.free_frames
    );

    match (memory::allocate_frame(), memory::allocate_frame()) {
        (Some(a), Some(b)) => {
            kprintln!("[memory] PMM self-test frames={:#x},{:#x}", a, b);
        }
        _ => {
            kprintln!("[memory] PMM self-test FAILED");
            cpu::halt_forever();
        }
    }

    let old_cr3 = cpu::read_cr3();
    let new_cr3 = paging::install_identity_4g();
    kprintln!(
        "[paging] Elyon CR3=OK old={:#018x} new={:#018x} identity_limit={} MiB",
        old_cr3,
        new_cr3,
        paging::identity_limit() / 1024 / 1024
    );

    if info.acpi_rsdp != 0 {
        kprintln!("[acpi] RSDP={:#018x}", info.acpi_rsdp);
    } else {
        kprintln!("[acpi] RSDP=not-found");
    }

    if framebuffer::draw_boot_screen(info, paging::identity_limit()) {
        kprintln!(
            "[display] framebuffer=OK {}x{} stride={}",
            info.framebuffer_width,
            info.framebuffer_height,
            info.framebuffer_stride
        );
    } else {
        kprintln!("[display] framebuffer unavailable/outside initial identity map");
    }

    kprintln!(
        "[cpu] CR0={:#018x} CR3={:#018x} CR4={:#018x}",
        cpu::read_cr0(),
        cpu::read_cr3(),
        cpu::read_cr4()
    );

    kprintln!("ELYON_KERNEL_READY_0_0_2");
    kprintln!("elyon> loader/kernel split active; scheduler not started");
    cpu::halt_forever()
}

fn validate_boot_info(info: &BootInfo) {
    if info.magic != BOOT_INFO_MAGIC {
        kprintln!("[kernel] BootInfo magic mismatch: {:#x}", info.magic);
        cpu::halt_forever();
    }
    if info.version != BOOT_INFO_VERSION {
        kprintln!("[kernel] BootInfo version mismatch: {}", info.version);
        cpu::halt_forever();
    }
    if info.boot_services_exited != 1 {
        kprintln!("[kernel] UEFI Boot Services still active");
        cpu::halt_forever();
    }
    if info.memory_map_valid != 1 {
        kprintln!("[kernel] memory map validation failed");
        cpu::halt_forever();
    }
    if info.kernel_stack_end > paging::identity_limit()
        || info.boot_info_phys_end > paging::identity_limit()
        || info.kernel_phys_end > paging::identity_limit()
    {
        kprintln!("[kernel] critical bootstrap allocation above 4 GiB");
        cpu::halt_forever();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    kprintln!("[KERNEL PANIC] {}", info);
    cpu::halt_forever()
}
