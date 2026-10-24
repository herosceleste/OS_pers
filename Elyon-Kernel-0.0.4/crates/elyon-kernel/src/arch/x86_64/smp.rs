use core::arch::global_asm;
use core::ptr::{copy_nonoverlapping, write_bytes, write_unaligned};
use core::sync::atomic::{AtomicU64, Ordering, fence};

use elyon_abi::BootInfo;

use crate::arch::x86_64::{apic, cpu, gdt, idt, percpu};
use crate::{kprintln, memory, timer};

pub const TRAMPOLINE_BASE: u64 = 0x0000_8000;
pub const TRAMPOLINE_END: u64 = TRAMPOLINE_BASE + 0x1000;
const BOOT_DATA: u64 = TRAMPOLINE_BASE + 0x0E00;
const GDTR_ADDR: u64 = TRAMPOLINE_BASE + 0x0F00;
const GDT_ADDR: u64 = TRAMPOLINE_BASE + 0x0F10;
const SIPI_VECTOR: u8 = (TRAMPOLINE_BASE >> 12) as u8;
const STARTUP_WAIT_MS: usize = 100;

pub const STABILITY_VECTOR: u8 = 0xF1;
pub const STABILITY_ROUNDS: u64 = 10;
const STABILITY_ROUND_DELAY_US: u32 = 20_000;

static STABILITY_IPI_ERRORS: AtomicU64 = AtomicU64::new(0);

const BOOT_CR3: u64 = BOOT_DATA;
const BOOT_STACK: u64 = BOOT_DATA + 0x08;
const BOOT_CPU_INDEX: u64 = BOOT_DATA + 0x10;
const BOOT_APIC_ID: u64 = BOOT_DATA + 0x18;
const BOOT_ENTRY: u64 = BOOT_DATA + 0x20;

global_asm!(
    r#"
.section .text.ap_trampoline,"ax"
.balign 16
.global elyon_ap_trampoline_start
elyon_ap_trampoline_start:
.code16
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    lgdt [0x8f00]

    mov eax, cr0
    or eax, 1
    mov cr0, eax

    .byte 0x66, 0xea
    .long 0x8000 + (elyon_ap_trampoline_pm32 - elyon_ap_trampoline_start)
    .word 0x08

elyon_ap_trampoline_pm32:
.code32
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax

    mov eax, cr4
    or eax, 0x20
    mov cr4, eax

    mov eax, dword ptr [0x8e00]
    mov cr3, eax

    mov ecx, 0xc0000080
    rdmsr
    or eax, 0x100
    wrmsr

    mov eax, cr0
    bts eax, 31
    mov cr0, eax

    .byte 0xea
    .long 0x8000 + (elyon_ap_trampoline_lm64 - elyon_ap_trampoline_start)
    .word 0x18

elyon_ap_trampoline_lm64:
.code64
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax

    mov rsp, qword ptr [0x8e08]
    xor rbp, rbp
    mov rdi, qword ptr [0x8e10]
    mov rsi, qword ptr [0x8e18]
    mov rax, qword ptr [0x8e20]
    call rax

1:
    cli
    hlt
    jmp 1b

.global elyon_ap_trampoline_end
elyon_ap_trampoline_end:
.code64
"#
);

unsafe extern "C" {
    static elyon_ap_trampoline_start: u8;
    static elyon_ap_trampoline_end: u8;
}

pub struct StartupSummary {
    pub cpu_index: usize,
    pub apic_id: u32,
    pub trampoline_base: u64,
    pub sipi_vector: u8,
    pub online_count: usize,
    pub prepared_count: usize,
}

pub struct AllStartupSummary {
    pub started: usize,
    pub online_count: usize,
    pub prepared_count: usize,
    pub trampoline_base: u64,
    pub sipi_vector: u8,
}

pub struct StabilitySummary {
    pub rounds: u64,
    pub online_count: usize,
    pub prepared_count: usize,
    pub ipi_errors: u64,
    pub min_ap_heartbeat: u64,
    pub max_ap_heartbeat: u64,
}

#[derive(Clone, Copy)]
pub enum SmpError {
    NoPreparedAp,
    TrampolinePageUnavailable,
    TrampolineTooLarge,
    InvalidCr3,
    TimerDelay,
    Apic,
    StartupTimeout,
    StabilityStateRegression,
    StabilityHeartbeatMissing,
    StabilityBootCountMismatch,
}

impl SmpError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoPreparedAp => "no prepared Application Processor",
            Self::TrampolinePageUnavailable => "trampoline page is not UEFI usable memory",
            Self::TrampolineTooLarge => "AP trampoline code exceeds low-memory code area",
            Self::InvalidCr3 => "Elyon CR3 does not fit AP bootstrap",
            Self::TimerDelay => "PIT delay failed during INIT-SIPI",
            Self::Apic => "Local APIC INIT-SIPI operation failed",
            Self::StartupTimeout => "Application Processor did not reach online state",
            Self::StabilityStateRegression => "SMP online/prepared state regressed",
            Self::StabilityHeartbeatMissing => "AP stability heartbeat missing",
            Self::StabilityBootCountMismatch => "CPU boot count changed during stability window",
        }
    }
}

pub fn start_all_prepared(info: &BootInfo) -> Result<AllStartupSummary, SmpError> {
    let mut started = 0usize;

    loop {
        let mut target = None;
        for index in 1..percpu::cpu_count() {
            if let Some(snapshot) = percpu::snapshot(index)
                && snapshot.state == percpu::CpuState::Prepared
            {
                target = Some((index, snapshot));
                break;
            }
        }

        let Some((cpu_index, snapshot)) = target else {
            break;
        };

        let summary = start_target(info, cpu_index, snapshot)?;
        started += 1;

        kprintln!(
            "[smp] trampoline={:#x} sipi_vector={:#x} cpu={} apic_id={} online={} prepared={}",
            summary.trampoline_base,
            summary.sipi_vector,
            summary.cpu_index,
            summary.apic_id,
            summary.online_count,
            summary.prepared_count
        );
    }

    if started == 0 {
        return Err(SmpError::NoPreparedAp);
    }

    Ok(AllStartupSummary {
        started,
        online_count: percpu::online_count(),
        prepared_count: percpu::prepared_count(),
        trampoline_base: TRAMPOLINE_BASE,
        sipi_vector: SIPI_VECTOR,
    })
}

pub fn qualify_stability() -> Result<StabilitySummary, SmpError> {
    let expected_cpus = percpu::cpu_count();
    if expected_cpus < 2 {
        return Err(SmpError::NoPreparedAp);
    }

    STABILITY_IPI_ERRORS.store(0, Ordering::Release);

    for round in 1..=STABILITY_ROUNDS {
        if percpu::online_count() != expected_cpus || percpu::prepared_count() != 0 {
            return Err(SmpError::StabilityStateRegression);
        }

        for index in 1..expected_cpus {
            let snapshot = percpu::snapshot(index).ok_or(SmpError::StabilityStateRegression)?;
            if snapshot.state != percpu::CpuState::Online {
                return Err(SmpError::StabilityStateRegression);
            }
            if snapshot.boot_count != 1 {
                return Err(SmpError::StabilityBootCountMismatch);
            }

            apic::send_fixed(snapshot.apic_id, STABILITY_VECTOR).map_err(|_| SmpError::Apic)?;
        }

        timer::pit_delay_us(STABILITY_ROUND_DELAY_US).map_err(|_| SmpError::TimerDelay)?;

        for index in 1..expected_cpus {
            let snapshot = percpu::snapshot(index).ok_or(SmpError::StabilityStateRegression)?;
            if snapshot.state != percpu::CpuState::Online
                || snapshot.boot_count != 1
                || snapshot.heartbeat < round
            {
                return Err(SmpError::StabilityHeartbeatMissing);
            }
        }
    }

    let mut min_ap_heartbeat = u64::MAX;
    let mut max_ap_heartbeat = 0u64;

    for index in 1..expected_cpus {
        let snapshot = percpu::snapshot(index).ok_or(SmpError::StabilityStateRegression)?;
        min_ap_heartbeat = min_ap_heartbeat.min(snapshot.heartbeat);
        max_ap_heartbeat = max_ap_heartbeat.max(snapshot.heartbeat);

        kprintln!(
            "[smp] stable cpu={} apic_id={} state={} heartbeat={} boot_count={}",
            snapshot.logical_id,
            snapshot.apic_id,
            snapshot.state.as_str(),
            snapshot.heartbeat,
            snapshot.boot_count
        );
    }

    let ipi_errors = STABILITY_IPI_ERRORS.load(Ordering::Acquire);
    if ipi_errors != 0 {
        return Err(SmpError::StabilityHeartbeatMissing);
    }

    Ok(StabilitySummary {
        rounds: STABILITY_ROUNDS,
        online_count: percpu::online_count(),
        prepared_count: percpu::prepared_count(),
        ipi_errors,
        min_ap_heartbeat,
        max_ap_heartbeat,
    })
}

fn start_target(
    info: &BootInfo,
    cpu_index: usize,
    target: percpu::CpuSnapshot,
) -> Result<StartupSummary, SmpError> {
    prepare_trampoline(info, cpu_index, target.apic_id, target.kernel_stack_end)?;

    // Le trampoline et ses paramètres sont écrits par des accès non volatiles :
    // ils doivent être globalement visibles avant que l'ICR ne réveille l'AP.
    fence(Ordering::SeqCst);

    apic::send_init(target.apic_id).map_err(|_| SmpError::Apic)?;
    timer::pit_delay_us(10_000).map_err(|_| SmpError::TimerDelay)?;

    apic::send_startup(target.apic_id, SIPI_VECTOR).map_err(|_| SmpError::Apic)?;
    timer::pit_delay_us(200).map_err(|_| SmpError::TimerDelay)?;

    if !percpu::is_online(cpu_index) {
        apic::send_startup(target.apic_id, SIPI_VECTOR).map_err(|_| SmpError::Apic)?;
    }

    for _ in 0..STARTUP_WAIT_MS {
        if percpu::is_online(cpu_index) {
            return Ok(StartupSummary {
                cpu_index,
                apic_id: target.apic_id,
                trampoline_base: TRAMPOLINE_BASE,
                sipi_vector: SIPI_VECTOR,
                online_count: percpu::online_count(),
                prepared_count: percpu::prepared_count(),
            });
        }
        timer::pit_delay_us(1_000).map_err(|_| SmpError::TimerDelay)?;
    }

    Err(SmpError::StartupTimeout)
}

fn prepare_trampoline(
    info: &BootInfo,
    cpu_index: usize,
    apic_id: u32,
    stack_top: u64,
) -> Result<(), SmpError> {
    if !memory::range_is_usable(info, TRAMPOLINE_BASE, TRAMPOLINE_END) {
        return Err(SmpError::TrampolinePageUnavailable);
    }

    let start = core::ptr::addr_of!(elyon_ap_trampoline_start) as usize;
    let end = core::ptr::addr_of!(elyon_ap_trampoline_end) as usize;
    let length = end.saturating_sub(start);

    if length == 0 || length > 0x0D00 {
        return Err(SmpError::TrampolineTooLarge);
    }

    let cr3 = cpu::read_cr3();
    if cr3 > u32::MAX as u64 {
        return Err(SmpError::InvalidCr3);
    }

    unsafe {
        write_bytes(TRAMPOLINE_BASE as *mut u8, 0, 0x1000);
        copy_nonoverlapping(start as *const u8, TRAMPOLINE_BASE as *mut u8, length);

        write_unaligned(BOOT_CR3 as *mut u64, cr3);
        write_unaligned(BOOT_STACK as *mut u64, stack_top);
        write_unaligned(BOOT_CPU_INDEX as *mut u64, cpu_index as u64);
        write_unaligned(BOOT_APIC_ID as *mut u64, apic_id as u64);
        write_unaligned(
            BOOT_ENTRY as *mut u64,
            elyon_ap_entry as *const () as usize as u64,
        );

        write_unaligned(GDTR_ADDR as *mut u16, (5 * 8 - 1) as u16);
        write_unaligned((GDTR_ADDR + 2) as *mut u32, GDT_ADDR as u32);

        let gdt = GDT_ADDR as *mut u64;
        gdt.add(0).write_unaligned(0x0000_0000_0000_0000);
        gdt.add(1).write_unaligned(0x00CF_9A00_0000_FFFF);
        gdt.add(2).write_unaligned(0x00CF_9200_0000_FFFF);
        gdt.add(3).write_unaligned(0x00AF_9A00_0000_FFFF);
        gdt.add(4).write_unaligned(0x00CF_9200_0000_FFFF);
    }

    Ok(())
}

#[unsafe(no_mangle)]
extern "sysv64" fn elyon_ap_entry(cpu_index: u64, expected_apic_id: u64) -> ! {
    cpu::disable_interrupts();

    let index = cpu_index as usize;
    // Avant toute prise de verrou ; l'index est confirmé plus bas par l'APIC ID
    // observé, vérifié contre celui attendu pour ce slot.
    super::cpu_local::init(index);
    let code_selector = gdt::load_for_cpu(index);
    idt::load();

    let observed_apic_id = match apic::init_secondary() {
        Ok(value) => value,
        Err(error) => {
            kprintln!(
                "[smp] AP FAILED cpu={} expected_apic_id={} reason={}",
                index,
                expected_apic_id,
                error.as_str()
            );
            cpu::halt_forever();
        }
    };

    if observed_apic_id != expected_apic_id as u32 {
        kprintln!(
            "[smp] AP FAILED cpu={} expected_apic_id={} observed_apic_id={}",
            index,
            expected_apic_id,
            observed_apic_id
        );
        cpu::halt_forever();
    }

    let expected_tr = gdt::tss_selector_for(index);
    let observed_tr = cpu::read_tr();
    if expected_tr == 0 || observed_tr != expected_tr {
        kprintln!(
            "[smp] AP FAILED cpu={} apic_id={} tr={:#x} expected_tr={:#x}",
            index,
            observed_apic_id,
            observed_tr,
            expected_tr
        );
        cpu::halt_forever();
    }

    if let Err(error) = percpu::mark_online(index, observed_apic_id) {
        kprintln!(
            "[smp] AP FAILED cpu={} apic_id={} reason={}",
            index,
            observed_apic_id,
            error.as_str()
        );
        cpu::halt_forever();
    }

    kprintln!(
        "[smp] AP online cpu={} apic_id={} selector={:#x} tr={:#x}",
        index,
        observed_apic_id,
        code_selector,
        observed_tr
    );
    kprintln!(
        "ELYON_AP_STARTED cpu={} apic_id={}",
        index,
        observed_apic_id
    );
    kprintln!("ELYON_AP_READY cpu={} apic_id={}", index, observed_apic_id);
    kprintln!("ELYON_AP_IDLE cpu={} apic_id={}", index, observed_apic_id);

    crate::sched::ap_enter(index)
}

#[unsafe(no_mangle)]
extern "C" fn elyon_smp_stability_ipi_dispatch() {
    let Ok(apic_id) = apic::current_id() else {
        STABILITY_IPI_ERRORS.fetch_add(1, Ordering::AcqRel);
        let _ = apic::eoi();
        return;
    };

    if !percpu::record_heartbeat_by_apic_id(apic_id) {
        STABILITY_IPI_ERRORS.fetch_add(1, Ordering::AcqRel);
    }

    if !apic::eoi() {
        STABILITY_IPI_ERRORS.fetch_add(1, Ordering::AcqRel);
    }
}
