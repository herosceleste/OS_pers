use core::arch::global_asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use elyon_abi::PAGE_SIZE;

use crate::arch::x86_64::{cpu, percpu};
use crate::{kprintln, memory};

const TASK_STACK_FRAMES: usize = 16;
const TASK_STACK_SIZE: u64 = TASK_STACK_FRAMES as u64 * PAGE_SIZE;
const INITIAL_RFLAGS: u64 = 0x2;
const CONTEXT_FRAME_QWORDS: u64 = 8;

static mut BSP_RSP: u64 = 0;
static mut TASK_RSP: u64 = 0;

static TASK_PHASE: AtomicU64 = AtomicU64::new(0);
static SWITCH_COUNT: AtomicU64 = AtomicU64::new(0);

global_asm!(
    r#"
.section .text.context_switch,"ax"
.balign 16

.global elyon_context_switch
elyon_context_switch:
    pushfq
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15

    mov qword ptr [rdi], rsp
    mov rsp, rsi

    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    popfq
    ret

.global elyon_context_task_trampoline
elyon_context_task_trampoline:
    call elyon_context_test_task_entry
    ud2
"#
);

unsafe extern "C" {
    fn elyon_context_switch(old_rsp: *mut u64, new_rsp: u64);
    fn elyon_context_task_trampoline();
}

pub struct ContextQualification {
    pub task_stack_start: u64,
    pub task_stack_end: u64,
    pub first_bsp_saved_rsp: u64,
    pub first_task_saved_rsp: u64,
    pub final_task_saved_rsp: u64,
    pub switches: u64,
    pub phases: u64,
    pub online_before: usize,
    pub online_after: usize,
}

#[derive(Clone, Copy)]
pub enum ContextError {
    AllocationFailed,
    StackOutsideIdentityMap,
    StackOverlap,
    InvalidInitialFrame,
    PhaseMismatch,
    BspRspOutsideKernelStack,
    TaskRspOutsideTaskStack,
    SwitchCountMismatch,
    SmpStateRegression,
}

impl ContextError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllocationFailed => "PMM could not allocate kernel task stack",
            Self::StackOutsideIdentityMap => "kernel task stack outside current identity map",
            Self::StackOverlap => "kernel task stack overlaps BSP stack",
            Self::InvalidInitialFrame => "invalid initial kernel task context frame",
            Self::PhaseMismatch => "kernel task did not resume at the expected phase",
            Self::BspRspOutsideKernelStack => "saved BSP RSP outside loader-provided kernel stack",
            Self::TaskRspOutsideTaskStack => "saved task RSP outside dedicated task stack",
            Self::SwitchCountMismatch => "unexpected context switch count",
            Self::SmpStateRegression => "SMP online/prepared state changed during context switch test",
        }
    }
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    expected_online: usize,
) -> Result<ContextQualification, ContextError> {
    cpu::disable_interrupts();

    let online_before = percpu::online_count();
    if online_before != expected_online || percpu::prepared_count() != 0 {
        return Err(ContextError::SmpStateRegression);
    }

    let task_stack_start =
        memory::allocate_contiguous_frames(TASK_STACK_FRAMES).ok_or(ContextError::AllocationFailed)?;
    let task_stack_end = task_stack_start + TASK_STACK_SIZE;

    if task_stack_end > identity_limit {
        return Err(ContextError::StackOutsideIdentityMap);
    }
    if ranges_overlap(
        task_stack_start,
        task_stack_end,
        bsp_stack_start,
        bsp_stack_end,
    ) {
        return Err(ContextError::StackOverlap);
    }

    unsafe {
        write_bytes(
            task_stack_start as *mut u8,
            0,
            TASK_STACK_SIZE as usize,
        );
    }

    TASK_PHASE.store(0, Ordering::Release);
    SWITCH_COUNT.store(0, Ordering::Release);

    unsafe {
        write_volatile(addr_of_mut!(BSP_RSP), 0);
        write_volatile(
            addr_of_mut!(TASK_RSP),
            build_initial_task_frame(task_stack_start, task_stack_end)?,
        );
    }

    switch_to_task();

    if TASK_PHASE.load(Ordering::Acquire) != 1 {
        return Err(ContextError::PhaseMismatch);
    }

    let first_bsp_saved_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };
    let first_task_saved_rsp = unsafe { read_volatile(addr_of!(TASK_RSP)) };

    if !inside_stack(first_bsp_saved_rsp, bsp_stack_start, bsp_stack_end) {
        return Err(ContextError::BspRspOutsideKernelStack);
    }
    if !inside_stack(first_task_saved_rsp, task_stack_start, task_stack_end) {
        return Err(ContextError::TaskRspOutsideTaskStack);
    }

    switch_to_task();

    let phases = TASK_PHASE.load(Ordering::Acquire);
    let switches = SWITCH_COUNT.load(Ordering::Acquire);
    let final_task_saved_rsp = unsafe { read_volatile(addr_of!(TASK_RSP)) };

    if phases != 2 {
        return Err(ContextError::PhaseMismatch);
    }
    if switches != 4 {
        return Err(ContextError::SwitchCountMismatch);
    }
    if !inside_stack(final_task_saved_rsp, task_stack_start, task_stack_end) {
        return Err(ContextError::TaskRspOutsideTaskStack);
    }

    let online_after = percpu::online_count();
    if online_after != expected_online || percpu::prepared_count() != 0 {
        return Err(ContextError::SmpStateRegression);
    }

    Ok(ContextQualification {
        task_stack_start,
        task_stack_end,
        first_bsp_saved_rsp,
        first_task_saved_rsp,
        final_task_saved_rsp,
        switches,
        phases,
        online_before,
        online_after,
    })
}

fn build_initial_task_frame(stack_start: u64, stack_end: u64) -> Result<u64, ContextError> {
    let frame_size = CONTEXT_FRAME_QWORDS * core::mem::size_of::<u64>() as u64;
    if stack_end <= stack_start || stack_end - stack_start < frame_size {
        return Err(ContextError::InvalidInitialFrame);
    }

    let frame = stack_end - frame_size;
    if frame & 0xF != 0 {
        return Err(ContextError::InvalidInitialFrame);
    }

    unsafe {
        let slots = frame as *mut u64;

        slots.add(0).write(0);
        slots.add(1).write(0);
        slots.add(2).write(0);
        slots.add(3).write(0);
        slots.add(4).write(0);
        slots.add(5).write(0);
        slots.add(6).write(INITIAL_RFLAGS);
        slots
            .add(7)
            .write(elyon_context_task_trampoline as *const () as usize as u64);
    }

    Ok(frame)
}

fn switch_to_task() {
    SWITCH_COUNT.fetch_add(1, Ordering::AcqRel);
    let task_rsp = unsafe { read_volatile(addr_of!(TASK_RSP)) };

    unsafe {
        elyon_context_switch(addr_of_mut!(BSP_RSP), task_rsp);
    }
}

fn switch_to_bsp() {
    SWITCH_COUNT.fetch_add(1, Ordering::AcqRel);
    let bsp_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };

    unsafe {
        elyon_context_switch(addr_of_mut!(TASK_RSP), bsp_rsp);
    }
}

fn inside_stack(rsp: u64, start: u64, end: u64) -> bool {
    rsp >= start && rsp < end
}

fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

#[unsafe(no_mangle)]
extern "C" fn elyon_context_test_task_entry() -> ! {
    TASK_PHASE.store(1, Ordering::Release);
    kprintln!(
        "[context] task phase=1 switches={} rsp={:#018x}",
        SWITCH_COUNT.load(Ordering::Acquire),
        cpu::read_rsp()
    );

    switch_to_bsp();

    TASK_PHASE.store(2, Ordering::Release);
    kprintln!(
        "[context] task phase=2 switches={} rsp={:#018x}",
        SWITCH_COUNT.load(Ordering::Acquire),
        cpu::read_rsp()
    );

    switch_to_bsp();

    kprintln!("[context] FAILED task resumed after terminal qualification phase");
    cpu::halt_forever()
}
