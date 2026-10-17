use core::arch::{asm, global_asm};
use core::ptr::addr_of_mut;

const IDT_LEN: usize = 256;
const KERNEL_CODE_SELECTOR: u16 = 0x08;
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

    fn new(handler: u64) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            options: INTERRUPT_GATE_PRESENT_RING0,
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

static mut IDT: [IdtEntry; IDT_LEN] = [IdtEntry::MISSING; IDT_LEN];

global_asm!(
    ".global elyon_exception_stub",
    "elyon_exception_stub:",
    "cli",
    "1:",
    "hlt",
    "jmp 1b",
);

unsafe extern "C" {
    fn elyon_exception_stub();
}

pub fn init() {
    let handler = elyon_exception_stub as *const () as usize as u64;
    let base = addr_of_mut!(IDT) as *mut IdtEntry;

    // Install a fail-closed handler for all architectural exceptions 0..31.
    // Elyon 0.0.2 will replace these guards with typed diagnostic handlers.
    for vector in 0..32usize {
        unsafe { base.add(vector).write(IdtEntry::new(handler)) };
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
