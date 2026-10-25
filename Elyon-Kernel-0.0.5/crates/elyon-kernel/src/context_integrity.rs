//! K004-05 : contexte CPU complet préservé à travers la préemption.
//!
//! Trois tâches préemptibles exécutent `elyon_context_integrity_loop` : chacune
//! charge un motif propre dans les quinze registres généraux, positionne DF et
//! revérifie le tout en boucle pendant que le timer la préempte. Un registre ou
//! un DF altéré par un switch fait sortir la boucle avec l'état observé ; le
//! noyau s'arrête alors fail-closed en nommant le registre corrompu. Les motifs
//! diffèrent d'une tâche à l'autre : une fuite de registre entre tâches est
//! détectée comme une corruption.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use elyon_core::cpu_context::{self, GPR_COUNT, RegisterFile};

use crate::arch::x86_64::cpu;
use crate::kprintln;
use crate::sched::{self, PREEMPT_TASKS, PreemptQualification, SchedError};

const CONTEXT_ARG_BASE: u64 = 0xE1_0405_0000;
/// Valeur de retour de la boucle quand un écart a été observé.
const CORRUPTED: u64 = u64::MAX;

global_asm!(
    r#"
.section .text.context_integrity,"ax"
.balign 16

// u64 elyon_context_integrity_loop(const u64 expected[15], u64 observed[16])
//
// Charge les quinze motifs attendus dans rax..r15, positionne DF, puis vérifie
// en boucle chaque registre et DF jusqu'à ce que ELYON_SCHED_STOP passe à 1.
// Aucun registre général ne reste libre pendant la boucle : les valeurs
// attendues sont relues dans une copie sur la pile de la tâche. Retourne le
// nombre d'itérations, ou u64::MAX après avoir écrit l'état observé
// (quinze registres puis RFLAGS) dans `observed` au premier écart.
//
// Cadre : [rsp+0..112] attendus, [rsp+120] itérations, [rsp+128] observed.
.global elyon_context_integrity_loop
elyon_context_integrity_loop:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    sub rsp, 144
    mov rax, qword ptr [rdi + 0]
    mov qword ptr [rsp + 0], rax
    mov rax, qword ptr [rdi + 8]
    mov qword ptr [rsp + 8], rax
    mov rax, qword ptr [rdi + 16]
    mov qword ptr [rsp + 16], rax
    mov rax, qword ptr [rdi + 24]
    mov qword ptr [rsp + 24], rax
    mov rax, qword ptr [rdi + 32]
    mov qword ptr [rsp + 32], rax
    mov rax, qword ptr [rdi + 40]
    mov qword ptr [rsp + 40], rax
    mov rax, qword ptr [rdi + 48]
    mov qword ptr [rsp + 48], rax
    mov rax, qword ptr [rdi + 56]
    mov qword ptr [rsp + 56], rax
    mov rax, qword ptr [rdi + 64]
    mov qword ptr [rsp + 64], rax
    mov rax, qword ptr [rdi + 72]
    mov qword ptr [rsp + 72], rax
    mov rax, qword ptr [rdi + 80]
    mov qword ptr [rsp + 80], rax
    mov rax, qword ptr [rdi + 88]
    mov qword ptr [rsp + 88], rax
    mov rax, qword ptr [rdi + 96]
    mov qword ptr [rsp + 96], rax
    mov rax, qword ptr [rdi + 104]
    mov qword ptr [rsp + 104], rax
    mov rax, qword ptr [rdi + 112]
    mov qword ptr [rsp + 112], rax
    mov qword ptr [rsp + 120], 0
    mov qword ptr [rsp + 128], rsi
    mov rax, qword ptr [rsp + 0]
    mov rbx, qword ptr [rsp + 8]
    mov rcx, qword ptr [rsp + 16]
    mov rdx, qword ptr [rsp + 24]
    mov rsi, qword ptr [rsp + 32]
    mov rdi, qword ptr [rsp + 40]
    mov rbp, qword ptr [rsp + 48]
    mov r8, qword ptr [rsp + 56]
    mov r9, qword ptr [rsp + 64]
    mov r10, qword ptr [rsp + 72]
    mov r11, qword ptr [rsp + 80]
    mov r12, qword ptr [rsp + 88]
    mov r13, qword ptr [rsp + 96]
    mov r14, qword ptr [rsp + 104]
    mov r15, qword ptr [rsp + 112]
    std
1:
    cmp rax, qword ptr [rsp + 0]
    jne 8f
    cmp rbx, qword ptr [rsp + 8]
    jne 8f
    cmp rcx, qword ptr [rsp + 16]
    jne 8f
    cmp rdx, qword ptr [rsp + 24]
    jne 8f
    cmp rsi, qword ptr [rsp + 32]
    jne 8f
    cmp rdi, qword ptr [rsp + 40]
    jne 8f
    cmp rbp, qword ptr [rsp + 48]
    jne 8f
    cmp r8, qword ptr [rsp + 56]
    jne 8f
    cmp r9, qword ptr [rsp + 64]
    jne 8f
    cmp r10, qword ptr [rsp + 72]
    jne 8f
    cmp r11, qword ptr [rsp + 80]
    jne 8f
    cmp r12, qword ptr [rsp + 88]
    jne 8f
    cmp r13, qword ptr [rsp + 96]
    jne 8f
    cmp r14, qword ptr [rsp + 104]
    jne 8f
    cmp r15, qword ptr [rsp + 112]
    jne 8f
    pushfq
    test qword ptr [rsp], 0x400
    lea rsp, [rsp + 8]
    jz 8f
    inc qword ptr [rsp + 120]
    cmp byte ptr [rip + ELYON_SCHED_STOP], 0
    je 1b
    cld
    mov rax, qword ptr [rsp + 120]
    jmp 9f
8:
    pushfq
    push rax
    mov rax, qword ptr [rsp + 144]
    mov qword ptr [rax + 8], rbx
    mov qword ptr [rax + 16], rcx
    mov qword ptr [rax + 24], rdx
    mov qword ptr [rax + 32], rsi
    mov qword ptr [rax + 40], rdi
    mov qword ptr [rax + 48], rbp
    mov qword ptr [rax + 56], r8
    mov qword ptr [rax + 64], r9
    mov qword ptr [rax + 72], r10
    mov qword ptr [rax + 80], r11
    mov qword ptr [rax + 88], r12
    mov qword ptr [rax + 96], r13
    mov qword ptr [rax + 104], r14
    mov qword ptr [rax + 112], r15
    mov rbx, qword ptr [rsp]
    mov qword ptr [rax + 0], rbx
    mov rbx, qword ptr [rsp + 8]
    mov qword ptr [rax + 120], rbx
    add rsp, 16
    cld
    mov rax, -1
9:
    add rsp, 144
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
"#
);

unsafe extern "C" {
    fn elyon_context_integrity_loop(expected: *const u64, observed: *mut u64) -> u64;
}

static EXPECTED: [RegisterFile; PREEMPT_TASKS] = [
    cpu_context::expected(0),
    cpu_context::expected(1),
    cpu_context::expected(2),
];
static ITERATIONS: [AtomicU64; PREEMPT_TASKS] = [const { AtomicU64::new(0) }; PREEMPT_TASKS];

pub struct ContextQualification {
    pub run: PreemptQualification,
    pub iterations: [u64; PREEMPT_TASKS],
    pub busy_preemptions: [u64; PREEMPT_TASKS],
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
) -> Result<ContextQualification, SchedError> {
    for counter in &ITERATIONS {
        counter.store(0, Ordering::Release);
    }

    let run = sched::run_preemptive(
        bsp_stack_start,
        bsp_stack_end,
        identity_limit,
        timer_initial_count,
        elyon_context_task_entry as *const () as usize as u64,
        CONTEXT_ARG_BASE,
    )?;

    let iterations = core::array::from_fn(|index| ITERATIONS[index].load(Ordering::Acquire));
    if iterations.contains(&0) {
        return Err(SchedError::NoProgress);
    }

    Ok(ContextQualification {
        run,
        iterations,
        busy_preemptions: sched::busy_preemptions(),
    })
}

#[unsafe(no_mangle)]
extern "C" fn elyon_context_task_entry(arg: u64) -> ! {
    let index = sched::preemptible_task_index(arg);
    let expected = &EXPECTED[index];
    let mut observed = [0u64; GPR_COUNT + 1];

    cpu::enable_interrupts();
    sched::set_busy(index, true);
    // SAFETY: `expected.gprs` contient GPR_COUNT u64 et `observed` GPR_COUNT + 1 ;
    // la boucle respecte l'ABI SysV (callee-saved restaurés, DF effacé au retour)
    // et n'utilise que la pile de la tâche courante.
    let iterations =
        unsafe { elyon_context_integrity_loop(expected.gprs.as_ptr(), observed.as_mut_ptr()) };
    sched::set_busy(index, false);

    if iterations == CORRUPTED {
        cpu::disable_interrupts();
        let mut gprs = [0u64; GPR_COUNT];
        gprs.copy_from_slice(&observed[..GPR_COUNT]);
        let observed = RegisterFile {
            gprs,
            rflags: observed[GPR_COUNT],
        };
        let report = cpu_context::compare(expected, &observed);
        match report.first_corrupted() {
            Some(gpr) => kprintln!(
                "[context-integrity] FAILED task_index={} register={} expected={:#018x} observed={:#018x} mask={:#06x} df_lost={}",
                index,
                gpr.name(),
                expected.gprs[gpr.index()],
                observed.gprs[gpr.index()],
                report.corrupted_mask,
                if report.df_lost { 1 } else { 0 }
            ),
            None => kprintln!(
                "[context-integrity] FAILED task_index={} df_lost={} rflags={:#x}",
                index,
                if report.df_lost { 1 } else { 0 },
                observed.rflags
            ),
        }
        cpu::halt_forever();
    }

    ITERATIONS[index].store(iterations, Ordering::Release);
    sched::add_progress(index, iterations);
    sched::exit_now()
}
