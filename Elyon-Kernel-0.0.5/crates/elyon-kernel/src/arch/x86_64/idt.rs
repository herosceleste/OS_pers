use core::arch::{asm, global_asm};
use core::ptr::{addr_of_mut, read_volatile};

use elyon_core::paranoid;
use elyon_core::user_access::{self, FaultSite};

use crate::arch::x86_64::{cpu, cpu_local, gdt};
use crate::kprintln;

const IDT_LEN: usize = 256;
const INTERRUPT_GATE_PRESENT_RING0: u16 = 0x8E00;
/// Porte d'interruption atteignable par `int` depuis le ring 3 (DPL 3).
const INTERRUPT_GATE_PRESENT_RING3: u16 = 0xEE00;
/// Sortie de la sonde ring 3 (K005-02).
pub const USER_PROBE_EXIT_VECTOR: u8 = 0x81;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    options: u16,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        options: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn new(handler: u64, selector: u16, ist: u16) -> Self {
        Self {
            offset_low: handler as u16,
            selector,
            options: INTERRUPT_GATE_PRESENT_RING0 | (ist & 0x7),
            offset_middle: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct Idtr {
    limit: u16,
    base: u64,
}

#[repr(C)]
struct ExceptionContext {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    vector: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    /// Toujours empilés en mode 64 bits, même sans changement de privilège.
    rsp: u64,
    ss: u64,
}

/// Cadre du stub paranoïaque : GS_BASE relu à l'entrée, puis le même cadre
/// que les autres exceptions.
#[repr(C)]
struct ParanoidContext {
    entry_gs_base: u64,
    frame: ExceptionContext,
}

const IA32_GS_BASE: u32 = 0xC000_0101;

static mut IDT: [IdtEntry; IDT_LEN] = [IdtEntry::MISSING; IDT_LEN];

global_asm!(
    r#"
// K005-02 : une entrée depuis le ring 3 (RPL 3 dans le CS sauvegardé, à
// `cs_offset` de RSP) échange GS_BASE et KERNEL_GS_BASE avant tout accès à la
// zone per-CPU ; la sortie vers le ring 3 les rééchange juste avant `iretq`.
// `lfence` sur les deux chemins de l'entrée : aucune lecture spéculative via
// GS ne part avec le mauvais GS (variante Spectre SWAPGS).
// K005-04 : `clac` en tête de chaque entrée. Une porte d'interruption
// n'efface pas RFLAGS.AC : sans lui, une IRQ venue du ring 3 (qui peut lever
// AC par `popfq`) ou de la fenêtre de copie ferait tourner le noyau avec SMAP
// levé. `iretq` rend au code interrompu son propre AC.
.macro SWAPGS_ON_ENTRY cs_offset
    clac
    test byte ptr [rsp + \cs_offset], 3
    jz .Lentry_from_kernel_\@
    swapgs
.Lentry_from_kernel_\@:
    lfence
.endm

// K005-03 : toute sortie vers le ring 3 passe par le seul `iretq` précédé de
// `swapgs` (elyon_user_iret), que le stub d'exception sait reconnaître.
.macro IRET_TO_ORIGIN cs_offset
    test byte ptr [rsp + \cs_offset], 3
    jnz elyon_return_to_user
    iretq
.endm

.macro EXC_NOERR n
.global elyon_exc_\n
elyon_exc_\n:
    push 0
    push \n
    jmp elyon_exception_common
.endm

.macro EXC_ERR n
.global elyon_exc_\n
elyon_exc_\n:
    push \n
    jmp elyon_exception_common
.endm

// K005-03 : NMI, #DF et #MC, chacun sur sa pile IST (`paranoid::ist_for_vector`).
.macro PARANOID_NOERR n
.global elyon_exc_\n
elyon_exc_\n:
    push 0
    push \n
    jmp elyon_paranoid_common
.endm

.macro PARANOID_ERR n
.global elyon_exc_\n
elyon_exc_\n:
    push \n
    jmp elyon_paranoid_common
.endm

EXC_NOERR 0
EXC_NOERR 1
PARANOID_NOERR 2
EXC_NOERR 3
EXC_NOERR 4
EXC_NOERR 5
EXC_NOERR 6
EXC_NOERR 7
PARANOID_ERR   8
EXC_NOERR 9
EXC_ERR   10
EXC_ERR   11
EXC_ERR   12
EXC_ERR   13
EXC_ERR   14
EXC_NOERR 15
EXC_NOERR 16
EXC_ERR   17
PARANOID_NOERR 18
EXC_NOERR 19
EXC_NOERR 20
EXC_ERR   21
EXC_NOERR 22
EXC_NOERR 23
EXC_NOERR 24
EXC_NOERR 25
EXC_NOERR 26
EXC_NOERR 27
EXC_NOERR 28
EXC_ERR   29
EXC_ERR   30
EXC_NOERR 31

.global elyon_apic_spurious
elyon_apic_spurious:
    iretq

.global elyon_smp_stability_ipi
elyon_smp_stability_ipi:
    SWAPGS_ON_ENTRY 8
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov r12, rsp
    and rsp, -16
    call elyon_smp_stability_ipi_dispatch
    mov rsp, r12
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    IRET_TO_ORIGIN 8

.global elyon_apic_timer_interrupt
elyon_apic_timer_interrupt:
    SWAPGS_ON_ENTRY 8
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov r12, rsp
    and rsp, -16
    mov rdi, qword ptr [r12 + 128]
    call elyon_timer_interrupt_dispatch
    mov rsp, r12
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    IRET_TO_ORIGIN 8

.global elyon_resched_ipi
elyon_resched_ipi:
    SWAPGS_ON_ENTRY 8
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov r12, rsp
    and rsp, -16
    mov rdi, qword ptr [r12 + 128]
    call elyon_resched_ipi_dispatch
    mov rsp, r12
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    IRET_TO_ORIGIN 8

// K005-05 : IPI de TLB shootdown, même cadre que les autres IPI.
.global elyon_tlb_ipi
elyon_tlb_ipi:
    SWAPGS_ON_ENTRY 8
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov r12, rsp
    and rsp, -16
    call elyon_tlb_ipi_dispatch
    mov rsp, r12
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    IRET_TO_ORIGIN 8

.global elyon_test_double_fault
elyon_test_double_fault:
    // K005-03 : le #DF part avec le GS utilisateur (KERNEL_GS_BASE préparé).
    swapgs
    mov rax, 0x0000001000000000
    mov rsp, rax
    int3
2:
    hlt
    jmp 2b

.global elyon_exception_common
elyon_exception_common:
    clac
    cli
    // [rsp] vecteur, +8 code d'erreur, +16 RIP, +24 CS.
    test byte ptr [rsp + 24], 3
    jnz .Lexception_swapgs
    // K005-03 : une faute de l'`iretq` de sortie vers le ring 3 (cadre refusé)
    // arrive avec un CS noyau, mais après `swapgs` : GS est déjà celui du
    // ring 3. Le RIP sauvegardé est alors exactement elyon_user_iret.
    push rax
    lea rax, [rip + elyon_user_iret]
    cmp qword ptr [rsp + 24], rax
    pop rax
    jne .Lexception_kernel_gs
.Lexception_swapgs:
    swapgs
.Lexception_kernel_gs:
    lfence
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    mov r12, rsp
    and rsp, -16
    call elyon_exception_dispatch
    // K005-04 : le dispatcher ne revient que pour une faute du noyau reprise
    // (copie utilisateur ou sonde), RIP réécrit vers un point de reprise du
    // noyau : retour sans `swapgs`, le CS sauvegardé est celui du noyau.
    mov rsp, r12
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq

// K005-02 : sortie de la sonde ring 3 (`int 0x81`, porte DPL 3). Le CS
// sauvegardé est passé au dispatcher, qui ne revient jamais : la tâche se
// termine sur sa pile noyau, abandonnée à l'entrée en ring 3.
.global elyon_user_probe_exit
elyon_user_probe_exit:
    SWAPGS_ON_ENTRY 8
    cld
    mov rdi, qword ptr [rsp + 8]
    mov rsi, qword ptr [rsp + 16]
    and rsp, -16
    call elyon_user_probe_exit_dispatch
2:
    hlt
    jmp 2b

// K005-03 : entrée « paranoïaque » de NMI, #DF et #MC. Ils arrivent
// n'importe où, y compris entre `swapgs` et `iretq` (CS noyau, GS
// utilisateur) : le CS sauvegardé ne dit rien de GS. Le stub relit le MSR
// GS_BASE, le compare au GS_BASE noyau rangé par `gdt::prepare_for_cpu` au
// sommet de la pile IST (à `slot` de RSP une fois le GS d'entrée poussé), et
// l'écrit s'il diffère ; il remet la valeur d'entrée avant `iretq`.
// KERNEL_GS_BASE n'est jamais touché : aucun `swapgs` ici.
// Invariant de l'IST : le dispatch ne fait ni `sti`, ni `iretq`, ni faute
// rattrapable ; sinon une NMI imbriquée réutiliserait la même pile et
// écraserait ce cadre. Pas de page de garde sous les piles IST (limite).
.global elyon_paranoid_common
elyon_paranoid_common:
    clac
    cld
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov ecx, {gs_base_msr}
    rdmsr
    shl rdx, 32
    or rax, rdx
    push rax
    mov rdx, qword ptr [rsp + {slot}]
    cmp rax, rdx
    je .Lparanoid_entry_gs_ok
    mov eax, edx
    shr rdx, 32
    wrmsr
.Lparanoid_entry_gs_ok:
    lfence
    mov r12, rsp
    mov rdi, rsp
    and rsp, -16
    call elyon_paranoid_dispatch
    mov rsp, r12
    // Le code interrompu (fenêtre `swapgs`/`iretq`, ring 3) reprend avec le
    // GS_BASE qu'il avait.
    mov rax, qword ptr [rsp]
    cmp rax, qword ptr [rsp + {slot}]
    je .Lparanoid_exit_gs_ok
    mov rdx, rax
    shr rdx, 32
    mov ecx, {gs_base_msr}
    wrmsr
.Lparanoid_exit_gs_ok:
    add rsp, 8
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq

// Unique retour vers le ring 3 : `swapgs` vers le GS utilisateur, `iretq`.
// Atteint IRQ masquées (porte d'interruption, `cli` d'elyon_enter_user) :
// aucune IRQ ne peut tomber entre les deux et entrer avec le GS utilisateur.
// Une NMI ou une #MC entre les deux est couverte par l'entrée paranoïaque ;
// une faute de l'`iretq` lui-même, par le test de RIP du stub d'exception.
.global elyon_return_to_user
elyon_return_to_user:
    swapgs
.global elyon_user_iret
elyon_user_iret:
    iretq

// void elyon_enter_user(const IretFrame *frame) -> !
// Entre en ring 3 : IRQ masquées jusqu'à `iretq`, `swapgs` vers le GS
// utilisateur (préparé dans KERNEL_GS_BASE), registres généraux effacés pour
// qu'aucune valeur du noyau ne parvienne au ring 3.
.global elyon_enter_user
elyon_enter_user:
    cli
    push qword ptr [rdi + 32]
    push qword ptr [rdi + 24]
    push qword ptr [rdi + 16]
    push qword ptr [rdi + 8]
    push qword ptr [rdi + 0]
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor ebp, ebp
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    jmp elyon_return_to_user
"#,
    gs_base_msr = const IA32_GS_BASE,
    slot = const paranoid::SLOT_OFFSET,
);

unsafe extern "C" {
    fn elyon_exc_0();
    fn elyon_exc_1();
    fn elyon_exc_2();
    fn elyon_exc_3();
    fn elyon_exc_4();
    fn elyon_exc_5();
    fn elyon_exc_6();
    fn elyon_exc_7();
    fn elyon_exc_8();
    fn elyon_exc_9();
    fn elyon_exc_10();
    fn elyon_exc_11();
    fn elyon_exc_12();
    fn elyon_exc_13();
    fn elyon_exc_14();
    fn elyon_exc_15();
    fn elyon_exc_16();
    fn elyon_exc_17();
    fn elyon_exc_18();
    fn elyon_exc_19();
    fn elyon_exc_20();
    fn elyon_exc_21();
    fn elyon_exc_22();
    fn elyon_exc_23();
    fn elyon_exc_24();
    fn elyon_exc_25();
    fn elyon_exc_26();
    fn elyon_exc_27();
    fn elyon_exc_28();
    fn elyon_exc_29();
    fn elyon_exc_30();
    fn elyon_exc_31();
    fn elyon_test_double_fault();
    fn elyon_apic_spurious();
    fn elyon_apic_timer_interrupt();
    fn elyon_smp_stability_ipi();
    fn elyon_resched_ipi();
    fn elyon_tlb_ipi();
    fn elyon_user_probe_exit();
    fn elyon_enter_user(frame: *const elyon_core::ring3::IretFrame) -> !;
    fn elyon_user_iret();
}

pub fn init(code_selector: u16) {
    let handlers = [
        elyon_exc_0 as *const () as usize as u64,
        elyon_exc_1 as *const () as usize as u64,
        elyon_exc_2 as *const () as usize as u64,
        elyon_exc_3 as *const () as usize as u64,
        elyon_exc_4 as *const () as usize as u64,
        elyon_exc_5 as *const () as usize as u64,
        elyon_exc_6 as *const () as usize as u64,
        elyon_exc_7 as *const () as usize as u64,
        elyon_exc_8 as *const () as usize as u64,
        elyon_exc_9 as *const () as usize as u64,
        elyon_exc_10 as *const () as usize as u64,
        elyon_exc_11 as *const () as usize as u64,
        elyon_exc_12 as *const () as usize as u64,
        elyon_exc_13 as *const () as usize as u64,
        elyon_exc_14 as *const () as usize as u64,
        elyon_exc_15 as *const () as usize as u64,
        elyon_exc_16 as *const () as usize as u64,
        elyon_exc_17 as *const () as usize as u64,
        elyon_exc_18 as *const () as usize as u64,
        elyon_exc_19 as *const () as usize as u64,
        elyon_exc_20 as *const () as usize as u64,
        elyon_exc_21 as *const () as usize as u64,
        elyon_exc_22 as *const () as usize as u64,
        elyon_exc_23 as *const () as usize as u64,
        elyon_exc_24 as *const () as usize as u64,
        elyon_exc_25 as *const () as usize as u64,
        elyon_exc_26 as *const () as usize as u64,
        elyon_exc_27 as *const () as usize as u64,
        elyon_exc_28 as *const () as usize as u64,
        elyon_exc_29 as *const () as usize as u64,
        elyon_exc_30 as *const () as usize as u64,
        elyon_exc_31 as *const () as usize as u64,
    ];

    let base = addr_of_mut!(IDT) as *mut IdtEntry;

    for (vector, handler) in handlers.iter().enumerate() {
        // NMI, #DF et #MC sur leur pile IST dédiée (K005-03).
        let ist = paranoid::ist_for_vector(vector as u8).unwrap_or(0);
        unsafe {
            base.add(vector)
                .write(IdtEntry::new(*handler, code_selector, ist));
        }
    }

    load();
}

pub fn load() {
    let base = addr_of_mut!(IDT) as *mut IdtEntry;
    let idtr = Idtr {
        limit: (core::mem::size_of::<[IdtEntry; IDT_LEN]>() - 1) as u16,
        base: base as u64,
    };

    unsafe {
        asm!(
            "lidt [{}]",
            in(reg) &idtr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Revient seulement pour une #PF du noyau reprise (K005-04), après avoir
/// réécrit RIP, et RSP pour la sonde SMEP, vers un point de reprise du
/// noyau ; toute autre exception est diagnostiquée puis arrête le système.
#[unsafe(no_mangle)]
extern "C" fn elyon_exception_dispatch(context: *mut ExceptionContext) {
    // SAFETY: le stub passe son propre cadre, sur la pile de ce CPU.
    let context = unsafe { &mut *context };
    if context.vector == user_access::VECTOR_PAGE_FAULT {
        let site = FaultSite {
            vector: context.vector,
            cs: context.cs,
            rip: context.rip,
            error_code: context.error_code,
            cr2: cpu::read_cr2(),
            rflags: context.rflags,
        };
        if let Some(resume) = crate::user_copy::resume(&site) {
            context.rip = resume;
            return;
        }
        if let Some((rip, rsp)) = crate::user_access_probe::on_fault(&site, context.rsp) {
            context.rip = rip;
            context.rsp = rsp;
            return;
        }
        if let Some(rip) = crate::user_memory_probe::on_fault(&site) {
            context.rip = rip;
            return;
        }
    }
    report(context);
    if context.rip == user_iret_address() {
        // K005-03 : faute de l'`iretq` vers le ring 3 ; le stub a refait
        // `swapgs`, ce diagnostic tourne donc avec le GS du noyau.
        let gs_base = read_gs_base();
        kprintln!(
            "[EXCEPTION] user iret fault gs_base={:#018x} kernel_gs={}",
            gs_base,
            u8::from(gs_base == cpu_local::area_address(cpu_local::index()))
        );
    }
    halt()
}

/// NMI, #DF et #MC (K005-03), entrés par `elyon_paranoid_common` avec le
/// GS_BASE noyau. Revient seulement pour un événement de la qualification ;
/// le stub remet alors le GS_BASE d'entrée avant `iretq`.
#[unsafe(no_mangle)]
extern "C" fn elyon_paranoid_dispatch(context: *const ParanoidContext) {
    // SAFETY: le stub passe son propre cadre, sur la pile IST de ce vecteur.
    let context = unsafe { &*context };
    let frame = &context.frame;
    if frame.vector == u64::from(paranoid::VECTOR_NMI) {
        // NMI d'arrêt système envoyé par un autre CPU : arrêt silencieux.
        crate::fatal::on_nmi();
    }
    if frame.vector != u64::from(paranoid::VECTOR_DOUBLE_FAULT)
        && crate::paranoid_probe::on_event(frame.vector, frame.rip, context.entry_gs_base)
    {
        return;
    }

    report(frame);
    let gs_base = read_gs_base();
    kprintln!(
        "[EXCEPTION] paranoid entry_gs={:#018x} gs_base={:#018x} replaced={}",
        context.entry_gs_base,
        gs_base,
        u8::from(context.entry_gs_base != gs_base)
    );
    if frame.vector == u64::from(paranoid::VECTOR_DOUBLE_FAULT) {
        let rsp = cpu::read_rsp();
        let (ist_start, ist_end) = gdt::double_fault_stack_span_for(cpu_local::index());
        let active = rsp >= ist_start && rsp < ist_end;
        kprintln!(
            "[EXCEPTION] #DF IST rsp={:#018x} span={:#018x}..{:#018x} active={}",
            rsp,
            ist_start,
            ist_end,
            if active { 1 } else { 0 }
        );
    }
    halt()
}

fn read_gs_base() -> u64 {
    // SAFETY: IA32_GS_BASE existe sur tout x86_64.
    unsafe { cpu::read_msr(IA32_GS_BASE) }
}

fn report(context: &ExceptionContext) {
    let name = exception_name(context.vector);

    kprintln!("");
    kprintln!("[EXCEPTION] vector={} {}", context.vector, name);
    kprintln!(
        "[EXCEPTION] error={:#x} rip={:#018x} cs={:#x} rflags={:#x}",
        context.error_code,
        context.rip,
        context.cs,
        context.rflags
    );
    kprintln!(
        "[EXCEPTION] rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
        context.rax,
        context.rbx,
        context.rcx,
        context.rdx
    );

    if context.vector == 14 {
        kprintln!("[EXCEPTION] CR2={:#018x}", cpu::read_cr2());
    }
}

fn halt() -> ! {
    kprintln!("[EXCEPTION] fail-closed halt");
    // Tous les CPU : un CPU arrêté seul pourrait tenir un verrou que les autres
    // attendraient sans fin. Un NMI inattendu passe aussi par ici.
    crate::fatal::halt_system()
}

fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "#DE divide error",
        1 => "#DB debug",
        2 => "NMI",
        3 => "#BP breakpoint",
        4 => "#OF overflow",
        5 => "#BR bound range",
        6 => "#UD invalid opcode",
        7 => "#NM device not available",
        8 => "#DF double fault",
        10 => "#TS invalid TSS",
        11 => "#NP segment not present",
        12 => "#SS stack segment fault",
        13 => "#GP general protection",
        14 => "#PF page fault",
        16 => "#MF x87 floating point",
        17 => "#AC alignment check",
        18 => "#MC machine check",
        19 => "#XM SIMD floating point",
        20 => "#VE virtualization",
        21 => "#CP control protection",
        29 => "#VC VMM communication",
        30 => "#SX security",
        _ => "reserved/unknown",
    }
}

pub fn trigger_double_fault_test() -> ! {
    unsafe { elyon_test_double_fault() }
    cpu::halt_forever()
}

pub fn install_interrupt_gate(vector: u8, code_selector: u16, handler: u64) {
    let base = addr_of_mut!(IDT) as *mut IdtEntry;
    unsafe {
        base.add(vector as usize)
            .write(IdtEntry::new(handler, code_selector, 0));
    }
}

/// Porte DPL 3 : la seule qu'un `int` du ring 3 puisse franchir (K005-02).
pub fn install_user_gate(vector: u8, code_selector: u16, handler: u64) {
    let base = addr_of_mut!(IDT) as *mut IdtEntry;
    let mut entry = IdtEntry::new(handler, code_selector, 0);
    entry.options = INTERRUPT_GATE_PRESENT_RING3;
    // SAFETY: entrée de l'IDT statique, déjà chargée par `lidt` ; appelé par
    // le BSP avant le démarrage des AP, donc avant que le vecteur puisse servir.
    unsafe { base.add(vector as usize).write(entry) };
}

/// Adresse de l'unique `iretq` de sortie vers le ring 3.
pub fn user_iret_address() -> u64 {
    elyon_user_iret as *const () as usize as u64
}

/// Stub installé pour un vecteur paranoïaque (NMI, #DF, #MC), 0 sinon.
pub fn paranoid_handler(vector: u8) -> u64 {
    match vector {
        2 => elyon_exc_2 as *const () as usize as u64,
        8 => elyon_exc_8 as *const () as usize as u64,
        18 => elyon_exc_18 as *const () as usize as u64,
        _ => 0,
    }
}

/// Porte `vector` relue dans l'IDT chargée sur ce CPU (`sidt`) : 64 bits bas
/// et hauts.
pub fn loaded_gate(vector: u8) -> Option<(u64, u64)> {
    let mut idtr = Idtr { limit: 0, base: 0 };
    // SAFETY: `sidt` écrit 10 octets dans `idtr`, sans autre effet.
    unsafe {
        asm!(
            "sidt [{}]",
            in(reg) &mut idtr,
            options(nostack, preserves_flags)
        );
    }
    let limit = idtr.limit;
    let offset = usize::from(vector) * 16;
    if usize::from(limit) < offset + 15 {
        return None;
    }
    let base = idtr.base as *const u64;
    // SAFETY: l'IDT chargée est la table statique du noyau, mappée en
    // identité ; la limite couvre cette porte.
    unsafe {
        let gate = base.add(usize::from(vector) * 2);
        Some((read_volatile(gate), read_volatile(gate.add(1))))
    }
}

pub fn user_probe_exit_handler() -> u64 {
    elyon_user_probe_exit as *const () as usize as u64
}

/// Entre en ring 3 avec `frame`, sans retour : la pile noyau courante est
/// abandonnée, la prochaine entrée depuis le ring 3 prendra RSP0.
///
/// # Safety
///
/// `frame` vient de `ring3::iret_frame` ; KERNEL_GS_BASE contient le GS
/// utilisateur ; RSP0 du TSS de ce CPU est le sommet de la pile noyau de la
/// tâche courante, et rien n'y est plus vivant.
pub unsafe fn enter_user(frame: &elyon_core::ring3::IretFrame) -> ! {
    // SAFETY: contrat reporté sur l'appelant.
    unsafe { elyon_enter_user(frame) }
}

pub fn apic_spurious_handler() -> u64 {
    elyon_apic_spurious as *const () as usize as u64
}

pub fn apic_timer_handler() -> u64 {
    elyon_apic_timer_interrupt as *const () as usize as u64
}

pub fn smp_stability_ipi_handler() -> u64 {
    elyon_smp_stability_ipi as *const () as usize as u64
}

pub fn resched_ipi_handler() -> u64 {
    elyon_resched_ipi as *const () as usize as u64
}

pub fn tlb_ipi_handler() -> u64 {
    elyon_tlb_ipi as *const () as usize as u64
}
