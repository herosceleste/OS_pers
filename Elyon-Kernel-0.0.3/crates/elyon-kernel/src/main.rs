#![no_main]
#![no_std]

mod acpi;
mod arch;
mod framebuffer;
mod memory;
mod serial;

use core::panic::PanicInfo;

use elyon_abi::{BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo};

use crate::arch::x86_64::{apic, cpu, gdt, idt, ioapic, paging};

const BUILD_ID: &str = match option_env!("ELYON_BUILD_ID") {
    Some(value) => value,
    None => "development",
};

const TEST_UD: bool = option_env!("ELYON_TEST_UD").is_some();
const TEST_DF: bool = option_env!("ELYON_TEST_DF").is_some();

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text._start")]
pub extern "sysv64" fn _start(info_ptr: *const BootInfo) -> ! {
    serial::init();
    cpu::disable_interrupts();

    kprintln!("ELYON KERNEL 0.0.3");
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
    kprintln!(
        "[arch] GDT=OK TSS=OK IST(#DF)=OK selector={:#x}",
        code_selector
    );

    idt::init(code_selector);
    kprintln!("[arch] IDT=OK exceptions=diagnostic");

    if TEST_UD {
        kprintln!("[test] injecting #UD via ud2");
        unsafe {
            core::arch::asm!("ud2", options(noreturn));
        }
    }

    let pmm = memory::init(info);
    kprintln!(
        "[memory] PMM=OK managed={} MiB frames={} free={}",
        pmm.managed_mib,
        pmm.managed_frames,
        pmm.free_frames
    );

    match (memory::allocate_frame(), memory::allocate_frame()) {
        (Some(a), Some(b)) if a >= 0x100000 && b >= 0x100000 => {
            kprintln!("[memory] PMM self-test frames={:#x},{:#x}", a, b);
        }
        (Some(a), Some(b)) => {
            kprintln!(
                "[memory] PMM self-test FAILED low-memory allocation={:#x},{:#x}",
                a,
                b
            );
            cpu::halt_forever();
        }
        _ => {
            kprintln!("[memory] PMM self-test FAILED");
            cpu::halt_forever();
        }
    }

    let (pt_start, pt_end) = paging::table_span();
    if pt_start < info.kernel_phys_start || pt_end > info.kernel_phys_end {
        kprintln!(
            "[paging] table ownership FAILED span={:#x}..{:#x} kernel={:#x}..{:#x}",
            pt_start,
            pt_end,
            info.kernel_phys_start,
            info.kernel_phys_end
        );
        cpu::halt_forever();
    }
    kprintln!("[paging] tables owned span={:#x}..{:#x}", pt_start, pt_end);

    let old_cr3 = cpu::read_cr3();
    let new_cr3 = paging::install_identity_4g();
    kprintln!(
        "[paging] Elyon CR3=OK old={:#018x} new={:#018x} identity_limit={} MiB",
        old_cr3,
        new_cr3,
        paging::identity_limit() / 1024 / 1024
    );

    if TEST_DF {
        kprintln!("[test] injecting #DF via invalid stack + int3");
        idt::trigger_double_fault_test();
    }

    let topology = match acpi::parse(info.acpi_rsdp, paging::identity_limit()) {
        Ok(topology) => topology,
        Err(error) => {
            kprintln!(
                "[acpi] FAILED rsdp={:#018x} reason={}",
                info.acpi_rsdp,
                error.as_str()
            );
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[acpi] RSDP=OK revision={} root={} address={:#018x}",
        topology.rsdp_revision,
        topology.root_kind.as_str(),
        topology.root_address
    );
    kprintln!(
        "[acpi] root={} entries={} validated={}",
        topology.root_kind.as_str(),
        topology.root_entries,
        topology.validated_table_count
    );
    kprintln!(
        "[acpi] MADT=OK address={:#018x} lapic={:#018x} flags={:#x}",
        topology.madt_address,
        topology.local_apic_address,
        topology.madt_flags
    );
    kprintln!(
        "[acpi] cpu_count={} usable={} ioapics={} isos={} nmis={}",
        topology.cpu_count(),
        topology.usable_cpu_count(),
        topology.io_apic_count(),
        topology.iso_count(),
        topology.nmi_count()
    );

    for cpu in topology.cpus() {
        kprintln!(
            "[acpi] cpu uid={} apic_id={} flags={:#x} x2apic={} enabled={} online_capable={}",
            cpu.acpi_uid,
            cpu.apic_id,
            cpu.flags,
            cpu.x2apic,
            cpu.enabled(),
            cpu.online_capable()
        );
    }

    for io_apic in topology.io_apics() {
        kprintln!(
            "[acpi] ioapic id={} address={:#018x} gsi_base={}",
            io_apic.id,
            io_apic.address,
            io_apic.gsi_base
        );
    }

    for iso in topology.isos() {
        kprintln!(
            "[acpi] iso bus={} source={} gsi={} flags={:#x}",
            iso.bus,
            iso.source,
            iso.gsi,
            iso.flags
        );
    }

    for nmi in topology.nmis() {
        kprintln!(
            "[acpi] nmi type={} uid={} gsi={} flags={:#x} lint={}",
            nmi.entry_type,
            nmi.acpi_uid,
            nmi.gsi,
            nmi.flags,
            nmi.lint
        );
    }

    kprintln!("ELYON_ACPI_READY");

    idt::install_interrupt_gate(
        apic::SPURIOUS_VECTOR,
        code_selector,
        idt::apic_spurious_handler(),
    );

    let lapic = match apic::init(&topology) {
        Ok(state) => state,
        Err(error) => {
            kprintln!("[apic] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[apic] cpuid apic={} x2apic={}",
        if lapic.cpuid_apic { 1 } else { 0 },
        if lapic.cpuid_x2apic { 1 } else { 0 }
    );
    kprintln!(
        "[apic] IA32_APIC_BASE={:#018x} madt_base={:#018x} mode={} bsp={}",
        lapic.msr_base,
        lapic.madt_base,
        lapic.mode.as_str(),
        if lapic.is_bsp { 1 } else { 0 }
    );
    kprintln!(
        "[apic] BSP apic_id={} version={:#x} madt_match=1",
        lapic.apic_id,
        lapic.version
    );
    kprintln!(
        "[apic] SVR vector={:#x} software_enable={}",
        apic::SPURIOUS_VECTOR,
        if lapic.software_enabled() { 1 } else { 0 }
    );

    if !apic::eoi() {
        kprintln!("[apic] EOI primitive FAILED");
        cpu::halt_forever();
    }
    kprintln!("[apic] EOI primitive=OK");
    kprintln!("ELYON_LAPIC_READY");

    let ioapic = match ioapic::init(&topology, lapic.apic_id) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[ioapic] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[ioapic] controllers={} bsp_apic_id={}",
        ioapic.controller_count(),
        lapic.apic_id
    );

    for controller in ioapic.controllers() {
        kprintln!(
            "[ioapic] madt_id={} hw_id={} version={:#x} arbitration_id={} address={:#018x} gsi={}..{} redirs={} id_match={}",
            controller.madt_id,
            controller.hw_id,
            controller.version,
            controller.arbitration_id,
            controller.address,
            controller.gsi_base,
            controller.gsi_end_exclusive() - 1,
            controller.redirection_entries,
            if controller.id_matches() { 1 } else { 0 }
        );
    }

    for route in ioapic.legacy_routes() {
        kprintln!(
            "[ioapic] route irq={} gsi={} vector={:#x} polarity={} trigger={} masked={} override={}",
            route.irq,
            route.gsi,
            route.vector,
            route.polarity_str(),
            route.trigger_str(),
            if route.masked { 1 } else { 0 },
            if route.overridden { 1 } else { 0 }
        );
    }

    kprintln!(
        "[ioapic] legacy_routes={} overrides_applied={} shadowed={} masked={}/{} all_masked={}",
        ioapic.legacy_route_count(),
        ioapic.overrides_applied,
        ioapic.shadowed_legacy_irqs,
        ioapic.masked_entries,
        ioapic.total_redirection_entries,
        if ioapic.all_masked() { 1 } else { 0 }
    );
    kprintln!("ELYON_IOAPIC_READY");

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

    kprintln!("ELYON_KERNEL_READY_0_0_3");
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
