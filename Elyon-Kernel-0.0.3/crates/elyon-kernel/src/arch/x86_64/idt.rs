use core::arch::{asm, global_asm};
use core::ptr::addr_of_mut;

use crate::arch::x86_64::{cpu, gdt};
use crate::kprintln;

const IDT_LEN: usize = 256;
const INTERRUPT_GATE_PRESENT_RING0: u16 = 0x8E00;

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
}

static mut IDT: [IdtEntry; IDT_LEN] = [IdtEntry::MISSING; IDT_LEN];

global_asm!(
    r#"
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

EXC_NOERR 0
EXC_NOERR 1
EXC_NOERR 2
EXC_NOERR 3
EXC_NOERR 4
EXC_NOERR 5
EXC_NOERR 6
EXC_NOERR 7
EXC_ERR   8
EXC_NOERR 9
EXC_ERR   10
EXC_ERR   11
EXC_ERR   12
EXC_ERR   13
EXC_ERR   14
EXC_NOERR 15
EXC_NOERR 16
EXC_ERR   17
EXC_NOERR 18
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

.global elyon_test_double_fault
elyon_test_double_fault:
    mov rax, 0x0000001000000000
    mov rsp, rax
    int3
2:
    hlt
    jmp 2b

.global elyon_exception_common
elyon_exception_common:
    cli
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
    and rsp, -16
    call elyon_exception_dispatch
1:
    hlt
    jmp 1b
"#
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
        let ist = if vector == 8 {
            gdt::DOUBLE_FAULT_IST_INDEX + 1
        } else {
            0
        };
        unsafe {
            base.add(vector)
                .write(IdtEntry::new(*handler, code_selector, ist));
        }
    }

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

#[unsafe(no_mangle)]
extern "C" fn elyon_exception_dispatch(context: *const ExceptionContext) -> ! {
    let context = unsafe { &*context };
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

    if context.vector == 8 {
        let rsp: u64;
        unsafe {
            asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
        }
        let (ist_start, ist_end) = gdt::double_fault_stack_span();
        let active = rsp >= ist_start && rsp < ist_end;
        kprintln!(
            "[EXCEPTION] #DF IST rsp={:#018x} span={:#018x}..{:#018x} active={}",
            rsp,
            ist_start,
            ist_end,
            if active { 1 } else { 0 }
        );
    }

    kprintln!("[EXCEPTION] fail-closed halt");
    cpu::halt_forever()
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

pub fn apic_spurious_handler() -> u64 {
    elyon_apic_spurious as *const () as usize as u64
}
