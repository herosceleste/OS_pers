//! K004-06 : isolation de l'état x87/SSE/AVX entre tâches sous préemption.
//!
//! Trois tâches préemptibles exécutent `elyon_simd_integrity_loop` : chacune
//! charge un MXCSR et des motifs XMM0..15 (et moitiés hautes YMM0..15 si AVX est
//! actif) qui lui sont propres, puis les revérifie en boucle pendant que le
//! timer la préempte. Sans sauvegarde par tâche, la tâche suivante écraserait
//! ces registres ; un écart fait sortir la boucle et le noyau s'arrête
//! fail-closed en nommant le registre corrompu.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use elyon_core::fpu::{self as simd, SimdFile};

use crate::arch::x86_64::cpu;
use crate::kprintln;
use crate::sched::{self, PREEMPT_TASKS, PreemptQualification, SchedError};

const FPU_ARG_BASE: u64 = 0xE1_0406_0000;
/// Valeur de retour de la boucle quand un écart a été observé.
const CORRUPTED: u64 = u64::MAX;

global_asm!(
    r#"
.section .text.fpu_integrity,"ax"
.balign 16

// u64 elyon_simd_integrity_loop(const SimdFile *expected, SimdFile *observed, u64 avx)
//
// Charge MXCSR et XMM0..15 (et, si `avx`, les moitiés hautes de YMM0..15) avec
// les motifs attendus, puis les revérifie à chaque itération jusqu'à ce que
// ELYON_SCHED_STOP passe à 1. Les registres SIMD sont tous occupés : chaque
// valeur est extraite dans un tampon de pile puis comparée en deux qwords.
// Retourne le nombre d'itérations, ou u64::MAX après avoir écrit l'état SIMD
// observé dans `observed` au premier écart.
//
// SimdFile : xmm[16] à +0, ymm_high[16] à +256, mxcsr à +512.
.global elyon_simd_integrity_loop
elyon_simd_integrity_loop:
    push rbx
    push r12
    push r13
    push r14
    push r15
    push rbp
    mov rbx, rdi
    mov r12, rsi
    mov r13, rdx
    sub rsp, 32
    xor r14, r14
    ldmxcsr dword ptr [rbx + 512]
    movdqu xmm0, xmmword ptr [rbx + 0]
    movdqu xmm1, xmmword ptr [rbx + 16]
    movdqu xmm2, xmmword ptr [rbx + 32]
    movdqu xmm3, xmmword ptr [rbx + 48]
    movdqu xmm4, xmmword ptr [rbx + 64]
    movdqu xmm5, xmmword ptr [rbx + 80]
    movdqu xmm6, xmmword ptr [rbx + 96]
    movdqu xmm7, xmmword ptr [rbx + 112]
    movdqu xmm8, xmmword ptr [rbx + 128]
    movdqu xmm9, xmmword ptr [rbx + 144]
    movdqu xmm10, xmmword ptr [rbx + 160]
    movdqu xmm11, xmmword ptr [rbx + 176]
    movdqu xmm12, xmmword ptr [rbx + 192]
    movdqu xmm13, xmmword ptr [rbx + 208]
    movdqu xmm14, xmmword ptr [rbx + 224]
    movdqu xmm15, xmmword ptr [rbx + 240]
    test r13, r13
    jz 1f
    vinsertf128 ymm0, ymm0, xmmword ptr [rbx + 256], 1
    vinsertf128 ymm1, ymm1, xmmword ptr [rbx + 272], 1
    vinsertf128 ymm2, ymm2, xmmword ptr [rbx + 288], 1
    vinsertf128 ymm3, ymm3, xmmword ptr [rbx + 304], 1
    vinsertf128 ymm4, ymm4, xmmword ptr [rbx + 320], 1
    vinsertf128 ymm5, ymm5, xmmword ptr [rbx + 336], 1
    vinsertf128 ymm6, ymm6, xmmword ptr [rbx + 352], 1
    vinsertf128 ymm7, ymm7, xmmword ptr [rbx + 368], 1
    vinsertf128 ymm8, ymm8, xmmword ptr [rbx + 384], 1
    vinsertf128 ymm9, ymm9, xmmword ptr [rbx + 400], 1
    vinsertf128 ymm10, ymm10, xmmword ptr [rbx + 416], 1
    vinsertf128 ymm11, ymm11, xmmword ptr [rbx + 432], 1
    vinsertf128 ymm12, ymm12, xmmword ptr [rbx + 448], 1
    vinsertf128 ymm13, ymm13, xmmword ptr [rbx + 464], 1
    vinsertf128 ymm14, ymm14, xmmword ptr [rbx + 480], 1
    vinsertf128 ymm15, ymm15, xmmword ptr [rbx + 496], 1
1:
    movdqu xmmword ptr [rsp], xmm0
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 0]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 8]
    jne 8f
    movdqu xmmword ptr [rsp], xmm1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 16]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 24]
    jne 8f
    movdqu xmmword ptr [rsp], xmm2
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 32]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 40]
    jne 8f
    movdqu xmmword ptr [rsp], xmm3
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 48]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 56]
    jne 8f
    movdqu xmmword ptr [rsp], xmm4
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 64]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 72]
    jne 8f
    movdqu xmmword ptr [rsp], xmm5
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 80]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 88]
    jne 8f
    movdqu xmmword ptr [rsp], xmm6
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 96]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 104]
    jne 8f
    movdqu xmmword ptr [rsp], xmm7
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 112]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 120]
    jne 8f
    movdqu xmmword ptr [rsp], xmm8
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 128]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 136]
    jne 8f
    movdqu xmmword ptr [rsp], xmm9
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 144]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 152]
    jne 8f
    movdqu xmmword ptr [rsp], xmm10
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 160]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 168]
    jne 8f
    movdqu xmmword ptr [rsp], xmm11
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 176]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 184]
    jne 8f
    movdqu xmmword ptr [rsp], xmm12
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 192]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 200]
    jne 8f
    movdqu xmmword ptr [rsp], xmm13
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 208]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 216]
    jne 8f
    movdqu xmmword ptr [rsp], xmm14
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 224]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 232]
    jne 8f
    movdqu xmmword ptr [rsp], xmm15
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 240]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 248]
    jne 8f
    test r13, r13
    jz 2f
    vextractf128 xmmword ptr [rsp], ymm0, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 256]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 264]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm1, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 272]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 280]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm2, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 288]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 296]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm3, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 304]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 312]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm4, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 320]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 328]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm5, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 336]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 344]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm6, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 352]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 360]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm7, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 368]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 376]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm8, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 384]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 392]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm9, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 400]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 408]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm10, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 416]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 424]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm11, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 432]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 440]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm12, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 448]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 456]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm13, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 464]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 472]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm14, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 480]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 488]
    jne 8f
    vextractf128 xmmword ptr [rsp], ymm15, 1
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rbx + 496]
    jne 8f
    mov rax, qword ptr [rsp + 8]
    cmp rax, qword ptr [rbx + 504]
    jne 8f
2:
    stmxcsr dword ptr [rsp]
    mov eax, dword ptr [rsp]
    cmp eax, dword ptr [rbx + 512]
    jne 8f
    inc r14
    cmp byte ptr [rip + ELYON_SCHED_STOP], 0
    je 1b
    mov rax, r14
    jmp 9f
8:
    movdqu xmmword ptr [r12 + 0], xmm0
    movdqu xmmword ptr [r12 + 16], xmm1
    movdqu xmmword ptr [r12 + 32], xmm2
    movdqu xmmword ptr [r12 + 48], xmm3
    movdqu xmmword ptr [r12 + 64], xmm4
    movdqu xmmword ptr [r12 + 80], xmm5
    movdqu xmmword ptr [r12 + 96], xmm6
    movdqu xmmword ptr [r12 + 112], xmm7
    movdqu xmmword ptr [r12 + 128], xmm8
    movdqu xmmword ptr [r12 + 144], xmm9
    movdqu xmmword ptr [r12 + 160], xmm10
    movdqu xmmword ptr [r12 + 176], xmm11
    movdqu xmmword ptr [r12 + 192], xmm12
    movdqu xmmword ptr [r12 + 208], xmm13
    movdqu xmmword ptr [r12 + 224], xmm14
    movdqu xmmword ptr [r12 + 240], xmm15
    test r13, r13
    jz 3f
    vextractf128 xmmword ptr [r12 + 256], ymm0, 1
    vextractf128 xmmword ptr [r12 + 272], ymm1, 1
    vextractf128 xmmword ptr [r12 + 288], ymm2, 1
    vextractf128 xmmword ptr [r12 + 304], ymm3, 1
    vextractf128 xmmword ptr [r12 + 320], ymm4, 1
    vextractf128 xmmword ptr [r12 + 336], ymm5, 1
    vextractf128 xmmword ptr [r12 + 352], ymm6, 1
    vextractf128 xmmword ptr [r12 + 368], ymm7, 1
    vextractf128 xmmword ptr [r12 + 384], ymm8, 1
    vextractf128 xmmword ptr [r12 + 400], ymm9, 1
    vextractf128 xmmword ptr [r12 + 416], ymm10, 1
    vextractf128 xmmword ptr [r12 + 432], ymm11, 1
    vextractf128 xmmword ptr [r12 + 448], ymm12, 1
    vextractf128 xmmword ptr [r12 + 464], ymm13, 1
    vextractf128 xmmword ptr [r12 + 480], ymm14, 1
    vextractf128 xmmword ptr [r12 + 496], ymm15, 1
3:
    stmxcsr dword ptr [r12 + 512]
    mov rax, -1
9:
    add rsp, 32
    pop rbp
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    ret
"#
);

unsafe extern "C" {
    fn elyon_simd_integrity_loop(
        expected: *const SimdFile,
        observed: *mut SimdFile,
        avx: u64,
    ) -> u64;
}

static EXPECTED: [SimdFile; PREEMPT_TASKS] = [
    simd::expected_simd(0, true),
    simd::expected_simd(1, true),
    simd::expected_simd(2, true),
];
static ITERATIONS: [AtomicU64; PREEMPT_TASKS] = [const { AtomicU64::new(0) }; PREEMPT_TASKS];
static AVX: AtomicU64 = AtomicU64::new(0);

pub struct FpuQualification {
    pub run: PreemptQualification,
    pub iterations: [u64; PREEMPT_TASKS],
    pub busy_preemptions: [u64; PREEMPT_TASKS],
    pub avx: bool,
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
    avx: bool,
) -> Result<FpuQualification, SchedError> {
    AVX.store(u64::from(avx), Ordering::Release);
    for counter in &ITERATIONS {
        counter.store(0, Ordering::Release);
    }

    let run = sched::run_preemptive(
        bsp_stack_start,
        bsp_stack_end,
        identity_limit,
        timer_initial_count,
        elyon_fpu_task_entry as *const () as usize as u64,
        FPU_ARG_BASE,
    )?;

    let iterations = core::array::from_fn(|index| ITERATIONS[index].load(Ordering::Acquire));
    if iterations.contains(&0) {
        return Err(SchedError::NoProgress);
    }

    Ok(FpuQualification {
        run,
        iterations,
        busy_preemptions: sched::busy_preemptions(),
        avx,
    })
}

#[unsafe(no_mangle)]
extern "C" fn elyon_fpu_task_entry(arg: u64) -> ! {
    let index = sched::preemptible_task_index(arg);
    let avx = AVX.load(Ordering::Acquire) != 0;
    let expected = &EXPECTED[index];
    let mut observed = SimdFile {
        xmm: [0; simd::SIMD_REGISTERS],
        ymm_high: [0; simd::SIMD_REGISTERS],
        mxcsr: 0,
    };

    cpu::enable_interrupts();
    sched::set_busy(index, true);
    // SAFETY: `expected` et `observed` sont des SimdFile repr(C) valides ; la
    // boucle respecte l'ABI SysV (callee-saved restaurés) et n'utilise que la
    // pile de la tâche ; les instructions AVX ne sont émises que si `avx`, c'est
    // à dire si XCR0 active le composant AVX.
    let iterations = unsafe { elyon_simd_integrity_loop(expected, &mut observed, u64::from(avx)) };
    sched::set_busy(index, false);

    if iterations == CORRUPTED {
        cpu::disable_interrupts();
        let report = simd::compare_simd(expected, &observed, avx);
        if let Some(reg) = report.first_corrupted_xmm() {
            kprintln!(
                "[fpu-integrity] FAILED task_index={} register=xmm{} expected={:#034x} observed={:#034x} xmm_mask={:#06x} ymm_mask={:#06x}",
                index,
                reg,
                expected.xmm[reg],
                observed.xmm[reg],
                report.xmm_mask,
                report.ymm_mask
            );
        } else if let Some(reg) = report.first_corrupted_ymm() {
            kprintln!(
                "[fpu-integrity] FAILED task_index={} register=ymm{}_high expected={:#034x} observed={:#034x} ymm_mask={:#06x}",
                index,
                reg,
                expected.ymm_high[reg],
                observed.ymm_high[reg],
                report.ymm_mask
            );
        } else {
            kprintln!(
                "[fpu-integrity] FAILED task_index={} register=mxcsr expected={:#x} observed={:#x}",
                index,
                expected.mxcsr,
                observed.mxcsr
            );
        }
        cpu::halt_forever();
    }

    ITERATIONS[index].store(iterations, Ordering::Release);
    sched::add_progress(index, iterations);
    sched::exit_now()
}
