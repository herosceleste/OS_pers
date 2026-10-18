use core::ptr::{addr_of, addr_of_mut};

use x86_64::VirtAddr;
use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct ExceptionStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut DOUBLE_FAULT_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);
static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut GDT: GlobalDescriptorTable<8> = GlobalDescriptorTable::<8>::empty();

pub fn init() -> u16 {
    unsafe {
        let stack_start = addr_of_mut!(DOUBLE_FAULT_STACK) as *mut u8;
        let stack_top = stack_start.add(DOUBLE_FAULT_STACK_SIZE) as u64;
        (*addr_of_mut!(TSS)).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(stack_top);

        let gdt = addr_of_mut!(GDT);
        let code = (*gdt).append(Descriptor::kernel_code_segment());
        let data = (*gdt).append(Descriptor::kernel_data_segment());
        let tss_descriptor = Descriptor::tss_segment_unchecked(addr_of!(TSS));
        let tss = (*gdt).append(tss_descriptor);

        (*gdt).load_unsafe();
        CS::set_reg(code);
        DS::set_reg(data);
        ES::set_reg(data);
        SS::set_reg(data);
        load_tss(tss);

        code.index() << 3
    }
}


pub fn double_fault_stack_span() -> (u64, u64) {
    let start = core::ptr::addr_of!(DOUBLE_FAULT_STACK) as u64;
    (start, start + DOUBLE_FAULT_STACK_SIZE as u64)
}
