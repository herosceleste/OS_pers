#![no_main]
#![no_std]

mod acpi;
mod arch;
mod context;
mod context_integrity;
mod fpu;
mod fpu_integrity;
mod framebuffer;
mod memory;
mod sched;
mod serial;
mod task;
mod task_stack;
mod timer;

use core::panic::PanicInfo;

use elyon_abi::{BOOT_INFO_MAGIC, BOOT_INFO_VERSION, BootInfo};

use crate::arch::x86_64::{apic, cpu, gdt, idt, ioapic, paging, percpu, smp};

const BUILD_ID: &str = match option_env!("ELYON_BUILD_ID") {
    Some(value) => value,
    None => "development",
};

const TEST_UD: bool = option_env!("ELYON_TEST_UD").is_some();
const TEST_DF: bool = option_env!("ELYON_TEST_DF").is_some();

/// Point d'entrée du noyau, atteint par saut depuis le loader UEFI.
///
/// # Safety
///
/// `info_ptr` doit être nul ou pointer vers un `BootInfo` initialisé, aligné,
/// mappé en identité et non modifié pendant la vie du noyau ; les UEFI Boot
/// Services doivent être terminés et les interruptions masquées.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text._start")]
pub unsafe extern "sysv64" fn _start(info_ptr: *const BootInfo) -> ! {
    serial::init();
    cpu::disable_interrupts();

    kprintln!("ELYON KERNEL 0.0.4");
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
        "[acpi] root={} entries={} validated={} skipped={}",
        topology.root_kind.as_str(),
        topology.root_entries,
        topology.validated_table_count,
        topology.skipped_table_count
    );
    if topology.skipped_table_count != 0 {
        kprintln!(
            "[acpi] WARN skipped={} non-essential table(s) with invalid checksum",
            topology.skipped_table_count
        );
    }
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

    let percpu_summary = match percpu::prepare(
        &topology,
        lapic.apic_id,
        info.kernel_stack_start,
        info.kernel_stack_end,
        info.kernel_phys_start,
        info.kernel_phys_end,
        paging::identity_limit(),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[percpu] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[percpu] cpus={} capacity={} bsp_logical={} bsp_apic_id={} unique={} owned={} tr_loaded={}",
        percpu_summary.cpu_count,
        crate::arch::x86_64::MAX_CPU_SLOTS,
        percpu_summary.bsp_index,
        percpu_summary.bsp_apic_id,
        if percpu_summary.resources_unique {
            1
        } else {
            0
        },
        if percpu_summary.resources_owned { 1 } else { 0 },
        if percpu_summary.bsp_tss_loaded { 1 } else { 0 }
    );

    for index in 0..percpu_summary.cpu_count {
        let Some(slot) = percpu::snapshot(index) else {
            kprintln!("[percpu] FAILED snapshot index={}", index);
            cpu::halt_forever();
        };

        kprintln!(
            "[percpu] cpu={} apic_id={} state={} stack={:#018x}..{:#018x} gdt={:#018x} tss={:#018x} ist={:#018x}..{:#018x} ticks={}",
            slot.logical_id,
            slot.apic_id,
            slot.state.as_str(),
            slot.kernel_stack_start,
            slot.kernel_stack_end,
            slot.gdt_address,
            slot.tss_address,
            slot.ist_start,
            slot.ist_end,
            slot.ticks
        );
    }

    kprintln!("ELYON_PERCPU_READY");

    // Les AP exécutent `sti` dès leur entrée : toute source d'IRQ externe doit
    // être masquée avant le premier INIT/SIPI (ARCHITECTURE §10).
    timer::mask_legacy_pic();
    let mut ioapic = match ioapic::discover_and_mask(&topology, info) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[ioapic] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };
    kprintln!(
        "[irq] pre_smp legacy_pic_masked=1 ioapic_masked={}/{}",
        ioapic.masked_entries,
        ioapic.total_redirection_entries
    );

    idt::install_interrupt_gate(
        smp::STABILITY_VECTOR,
        code_selector,
        idt::smp_stability_ipi_handler(),
    );

    if percpu_summary.cpu_count > 1 {
        let smp_summary = match smp::start_all_prepared(info) {
            Ok(summary) => summary,
            Err(error) => {
                kprintln!("[smp] FAILED reason={}", error.as_str());
                cpu::halt_forever();
            }
        };

        if smp_summary.online_count != percpu_summary.cpu_count || smp_summary.prepared_count != 0 {
            kprintln!(
                "[smp] FAILED final_state started={} online={} prepared={} expected={}",
                smp_summary.started,
                smp_summary.online_count,
                smp_summary.prepared_count,
                percpu_summary.cpu_count
            );
            cpu::halt_forever();
        }

        kprintln!(
            "[smp] all_ready started={} online={} prepared={} expected={} trampoline={:#x} sipi_vector={:#x}",
            smp_summary.started,
            smp_summary.online_count,
            smp_summary.prepared_count,
            percpu_summary.cpu_count,
            smp_summary.trampoline_base,
            smp_summary.sipi_vector
        );
        kprintln!("ELYON_AP_BOOT_READY");
        kprintln!("ELYON_SMP_READY");

        let stability = match smp::qualify_stability() {
            Ok(summary) => summary,
            Err(error) => {
                kprintln!("[smp] STABILITY FAILED reason={}", error.as_str());
                cpu::halt_forever();
            }
        };

        kprintln!(
            "[smp] stability rounds={} online={} prepared={} ipi_errors={} min_heartbeat={} max_heartbeat={}",
            stability.rounds,
            stability.online_count,
            stability.prepared_count,
            stability.ipi_errors,
            stability.min_ap_heartbeat,
            stability.max_ap_heartbeat
        );
        kprintln!("ELYON_SMP_STABLE");
    } else {
        kprintln!("[smp] no AP discovered; bootstrap skipped");
    }

    let context_qualification = match context::qualify(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
        percpu::online_count(),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[context] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[context] task_stack={:#018x}..{:#018x} size={} KiB source=PMM",
        context_qualification.task_stack_start,
        context_qualification.task_stack_end,
        (context_qualification.task_stack_end - context_qualification.task_stack_start) / 1024
    );
    kprintln!(
        "[context] saved bsp_rsp={:#018x} task_rsp_first={:#018x} task_rsp_final={:#018x}",
        context_qualification.first_bsp_saved_rsp,
        context_qualification.first_task_saved_rsp,
        context_qualification.final_task_saved_rsp
    );
    kprintln!(
        "[context] result phases={} switches={} online_before={} online_after={} prepared={}",
        context_qualification.phases,
        context_qualification.switches,
        context_qualification.online_before,
        context_qualification.online_after,
        percpu::prepared_count()
    );
    kprintln!("ELYON_CONTEXT_SWITCH_READY");

    let task_model = match task::qualify_model() {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[task] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    if task_model.states_seen_mask != task::required_states_mask() {
        kprintln!(
            "[task] FAILED states_seen={:#x} required={:#x}",
            task_model.states_seen_mask,
            task::required_states_mask()
        );
        cpu::halt_forever();
    }

    kprintln!(
        "[task] ids={},{} unique={} capacity={} created={} transitions={} invalid_rejected={} states_seen={:#x}",
        task_model.first_id,
        task_model.second_id,
        if task_model.unique_ids { 1 } else { 0 },
        task::MAX_TASKS,
        task_model.created_total,
        task_model.successful_transitions,
        task_model.invalid_transitions_rejected,
        task_model.states_seen_mask
    );
    kprintln!("[task] states=READY,RUNNING,BLOCKED,SLEEPING,DEAD affinity_enforced=1");
    kprintln!("ELYON_TASK_MODEL_READY");

    let stacks = match task_stack::qualify(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[task-stack] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    for report in &stacks.tasks {
        kprintln!(
            "[task-stack] task={} stack={:#018x}..{:#018x} size={} KiB initial_rsp={:#018x} observed_rsp={:#018x} saved_rsp={:#018x} arg={:#x} canary=OK",
            report.id,
            report.stack.start,
            report.stack.end,
            report.stack.size() / 1024,
            report.initial_rsp,
            report.observed_rsp,
            report.saved_rsp,
            report.arg
        );
    }
    kprintln!(
        "[task-stack] tasks={} distinct=1 overlaps=0 owned={} rounds={} switches={} args_ok={} canaries_ok={}",
        task_stack::QUALIFY_TASKS,
        stacks.owned,
        task_stack::ROUNDS,
        stacks.switches,
        stacks.args_ok,
        stacks.canaries_ok
    );
    kprintln!("ELYON_TASK_STACKS_READY");

    let fpu_setup = match fpu::init() {
        Ok(setup) => setup,
        Err(error) => {
            kprintln!("[fpu] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };
    kprintln!(
        "[fpu] fxsr={} sse2={} xsave={} avx={} xcr0_supported={:#x} strategy={} xcr0={:#x} avx_enabled={} area={} align={} forced_fxsave={} cr0={:#x} cr4={:#x}",
        if fpu_setup.features.fxsr { 1 } else { 0 },
        if fpu_setup.features.sse2 { 1 } else { 0 },
        if fpu_setup.features.xsave { 1 } else { 0 },
        if fpu_setup.features.avx { 1 } else { 0 },
        fpu_setup.features.xcr0_supported,
        fpu_setup.strategy.as_str(),
        match fpu_setup.strategy {
            elyon_core::fpu::Strategy::Xsave { xcr0 } => xcr0,
            elyon_core::fpu::Strategy::Fxsave => 0,
        },
        if fpu_setup.strategy.avx() { 1 } else { 0 },
        fpu_setup.layout.size,
        fpu_setup.layout.align,
        if fpu_setup.forced_fxsave { 1 } else { 0 },
        fpu_setup.cr0,
        fpu_setup.cr4
    );

    let rr = match sched::qualify(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[sched] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    let t = &rr.trace;
    kprintln!(
        "[sched] tasks={},{},{} trace={},{},{},{},{},{},{},{},{},{},{},{}",
        rr.ids[0],
        rr.ids[1],
        rr.ids[2],
        t[0],
        t[1],
        t[2],
        t[3],
        t[4],
        t[5],
        t[6],
        t[7],
        t[8],
        t[9],
        t[10],
        t[11]
    );
    kprintln!(
        "[sched] policy=round-robin cooperative iterations={} yields={} exits={} task_switches={} bsp_switches={} running_max={} args_ok={} rsp_ok={} canaries_ok={} queue_empty=1",
        sched::ITERATIONS,
        rr.yields,
        rr.exits,
        rr.task_switches,
        rr.bsp_switches,
        rr.running_max,
        rr.args_ok,
        rr.rsp_ok,
        rr.canaries_ok
    );
    kprintln!("ELYON_SCHED_RR_READY");

    if let Err(error) = ioapic::route_legacy(&mut ioapic, &topology, lapic.apic_id) {
        kprintln!("[ioapic] FAILED reason={}", error.as_str());
        cpu::halt_forever();
    }

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
        if !controller.id_matches() {
            kprintln!(
                "[ioapic] WARNING id_mismatch madt_id={} hw_id={} policy=diagnostic-only",
                controller.madt_id,
                controller.hw_id
            );
        }
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
        "[ioapic] legacy_routes={} overrides_applied={} shadowed={} id_mismatches={} masked={}/{} all_masked={}",
        ioapic.legacy_route_count(),
        ioapic.overrides_applied,
        ioapic.shadowed_legacy_irqs,
        ioapic.id_mismatches,
        ioapic.masked_entries,
        ioapic.total_redirection_entries,
        if ioapic.all_masked() { 1 } else { 0 }
    );
    kprintln!("ELYON_IOAPIC_READY");

    idt::install_interrupt_gate(
        timer::TIMER_VECTOR,
        code_selector,
        idt::apic_timer_handler(),
    );

    let timer_calibration = match timer::calibrate_and_start() {
        Ok(calibration) => calibration,
        Err(error) => {
            kprintln!("[timer] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[timer] source={} pit_count={} lapic_elapsed={} divide={} effective_hz={} initial_count={} target_hz={}",
        timer_calibration.source.as_str(),
        timer_calibration.pit_ticks,
        timer_calibration.lapic_elapsed,
        timer_calibration.divide,
        timer_calibration.effective_lapic_hz,
        timer_calibration.initial_count,
        timer_calibration.target_hz
    );
    kprintln!(
        "[timer] vector={:#x} legacy_pic_masked=1 qualification_target={}",
        timer::TIMER_VECTOR,
        timer::QUALIFICATION_TICKS
    );

    if let Some(pit_failure) = timer_calibration.pit_failure {
        kprintln!(
            "[timer] WARN pit_calibration_failed reason={} fallback=CPUID40000010",
            pit_failure.as_str()
        );
    }

    cpu::enable_interrupts();
    let timer_ticks = timer::wait_for_ticks(timer::QUALIFICATION_TICKS);
    cpu::disable_interrupts();
    let timer_ticks = match timer_ticks {
        Ok(ticks) => ticks,
        Err(error) => {
            kprintln!(
                "[timer] FAILED reason={} ticks={}",
                error.as_str(),
                timer::ticks()
            );
            cpu::halt_forever();
        }
    };

    if let Err(error) = timer::stop() {
        kprintln!("[timer] FAILED reason={}", error.as_str());
        cpu::halt_forever();
    }

    let bsp_percpu_ticks = percpu::bsp_ticks();
    if bsp_percpu_ticks != timer_ticks {
        kprintln!(
            "[percpu] FAILED bsp_ticks={} timer_ticks={}",
            bsp_percpu_ticks,
            timer_ticks
        );
        cpu::halt_forever();
    }

    kprintln!(
        "[timer] ticks={} eoi_failures={} periodic=1",
        timer_ticks,
        timer::eoi_failures()
    );
    kprintln!(
        "[percpu] bsp_ticks={} timer_ticks={} match=1",
        bsp_percpu_ticks,
        timer_ticks
    );
    kprintln!("ELYON_TIMER_READY");

    let preemption = match sched::qualify_preemption(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
        timer_calibration.initial_count,
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[preempt] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprint!(
        "[preempt] tasks={},{},{} trace=",
        preemption.ids[0],
        preemption.ids[1],
        preemption.ids[2]
    );
    for (index, task_id) in preemption.trace.iter().enumerate() {
        if index == 0 {
            kprint!("{}", task_id);
        } else {
            kprint!(",{}", task_id);
        }
    }
    kprintln!();
    kprintln!(
        "[preempt] policy=timer-preemptive quantum_ticks={} target={} preemptions={} task_ticks={} min_slices={} counters={},{},{} exits={} running_max={} eoi_failures={} canaries_ok={}",
        sched::QUANTUM_TICKS,
        sched::PREEMPT_TARGET,
        preemption.preemptions,
        preemption.task_ticks,
        preemption.min_slices,
        preemption.counters[0],
        preemption.counters[1],
        preemption.counters[2],
        preemption.exits,
        preemption.running_max,
        timer::eoi_failures(),
        preemption.canaries_ok
    );
    kprintln!("ELYON_SCHED_PREEMPT_READY");

    let integrity = match context_integrity::qualify(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
        timer_calibration.initial_count,
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[context-integrity] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[context-integrity] tasks={},{},{} registers=15 df=1 preemptions={} min_slices={} busy_preemptions={},{},{} iterations={},{},{} corruptions=0 exits={} running_max={} eoi_failures={} canaries_ok={}",
        integrity.run.ids[0],
        integrity.run.ids[1],
        integrity.run.ids[2],
        integrity.run.preemptions,
        integrity.run.min_slices,
        integrity.busy_preemptions[0],
        integrity.busy_preemptions[1],
        integrity.busy_preemptions[2],
        integrity.iterations[0],
        integrity.iterations[1],
        integrity.iterations[2],
        integrity.run.exits,
        integrity.run.running_max,
        timer::eoi_failures(),
        integrity.run.canaries_ok
    );
    kprintln!("ELYON_SCHED_CONTEXT_READY");

    let simd = match fpu_integrity::qualify(
        info.kernel_stack_start,
        info.kernel_stack_end,
        paging::identity_limit(),
        timer_calibration.initial_count,
        fpu_setup.strategy.avx(),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            kprintln!("[fpu-integrity] FAILED reason={}", error.as_str());
            cpu::halt_forever();
        }
    };

    kprintln!(
        "[fpu-integrity] tasks={},{},{} strategy={} avx={} xmm=16 ymm_high={} mxcsr=1 preemptions={} min_slices={} busy_preemptions={},{},{} iterations={},{},{} corruptions=0 exits={} running_max={} eoi_failures={} canaries_ok={}",
        simd.run.ids[0],
        simd.run.ids[1],
        simd.run.ids[2],
        fpu_setup.strategy.as_str(),
        if simd.avx { 1 } else { 0 },
        if simd.avx { 16 } else { 0 },
        simd.run.preemptions,
        simd.run.min_slices,
        simd.busy_preemptions[0],
        simd.busy_preemptions[1],
        simd.busy_preemptions[2],
        simd.iterations[0],
        simd.iterations[1],
        simd.iterations[2],
        simd.run.exits,
        simd.run.running_max,
        timer::eoi_failures(),
        simd.run.canaries_ok
    );
    kprintln!("ELYON_SCHED_FPU_READY");

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

    kprintln!("ELYON_KERNEL_READY_0_0_4");
    kprintln!(
        "elyon> loader/kernel split active; context-switch base active; scheduler not started"
    );
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
