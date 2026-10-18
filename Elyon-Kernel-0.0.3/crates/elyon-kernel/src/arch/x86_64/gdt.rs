use core::ptr::{addr_of, addr_of_mut};

use x86_64::VirtAddr;
use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;

use super::MAX_CPU_SLOTS;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct ExceptionStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut DOUBLE_FAULT_STACKS: [ExceptionStack; MAX_CPU_SLOTS] =
    [const { ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]) }; MAX_CPU_SLOTS];
static mut TSS_TABLE: [TaskStateSegment; MAX_CPU_SLOTS] =
    [const { TaskStateSegment::new() }; MAX_CPU_SLOTS];
static mut GDT_TABLE: [GlobalDescriptorTable<8>; MAX_CPU_SLOTS] =
    [const { GlobalDescriptorTable::<8>::empty() }; MAX_CPU_SLOTS];
static mut TSS_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];

pub fn init() -> u16 {
    init_for_cpu(0)
}

pub fn init_for_cpu(cpu_index: usize) -> u16 {
    let (code, data, tss) = build_for_cpu(cpu_index);

    unsafe {
        let gdt_table = addr_of_mut!(GDT_TABLE);
        (*gdt_table)[cpu_index].load_unsafe();
        CS::set_reg(code);
        DS::set_reg(data);
        ES::set_reg(data);
        SS::set_reg(data);
        load_tss(tss);
    }

    code.index() << 3
}

pub fn prepare_for_cpu(cpu_index: usize) {
    let _ = build_for_cpu(cpu_index);
}

fn build_for_cpu(
    cpu_index: usize,
) -> (
    x86_64::structures::gdt::SegmentSelector,
    x86_64::structures::gdt::SegmentSelector,
    x86_64::structures::gdt::SegmentSelector,
) {
    assert!(cpu_index < MAX_CPU_SLOTS);

    unsafe {
        let stacks = addr_of_mut!(DOUBLE_FAULT_STACKS);
        let stack_start = addr_of_mut!((*stacks)[cpu_index]) as *mut u8;
        let stack_top = stack_start.add(DOUBLE_FAULT_STACK_SIZE) as u64;

        let tss_table = addr_of_mut!(TSS_TABLE);
        (*tss_table)[cpu_index] = TaskStateSegment::new();
        (*tss_table)[cpu_index].interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(stack_top);

        let gdt_table = addr_of_mut!(GDT_TABLE);
        (*gdt_table)[cpu_index] = GlobalDescriptorTable::<8>::empty();

        let code = (*gdt_table)[cpu_index].append(Descriptor::kernel_code_segment());
        let data = (*gdt_table)[cpu_index].append(Descriptor::kernel_data_segment());
        let tss_ptr = addr_of!((*tss_table)[cpu_index]);
        let tss_descriptor = Descriptor::tss_segment_unchecked(tss_ptr);
        let tss = (*gdt_table)[cpu_index].append(tss_descriptor);
        TSS_SELECTORS[cpu_index] = tss.index() << 3;

        (code, data, tss)
    }
}

pub fn double_fault_stack_span() -> (u64, u64) {
    double_fault_stack_span_for(0)
}

pub fn double_fault_stack_span_for(cpu_index: usize) -> (u64, u64) {
    assert!(cpu_index < MAX_CPU_SLOTS);
    let stacks = addr_of!(DOUBLE_FAULT_STACKS);
    let start = unsafe { addr_of!((*stacks)[cpu_index]) as u64 };
    (start, start + DOUBLE_FAULT_STACK_SIZE as u64)
}

pub fn gdt_address(cpu_index: usize) -> u64 {
    assert!(cpu_index < MAX_CPU_SLOTS);
    let table = addr_of!(GDT_TABLE);
    unsafe { addr_of!((*table)[cpu_index]) as u64 }
}

pub fn tss_address(cpu_index: usize) -> u64 {
    assert!(cpu_index < MAX_CPU_SLOTS);
    let table = addr_of!(TSS_TABLE);
    unsafe { addr_of!((*table)[cpu_index]) as u64 }
}


pub fn tss_selector_for(cpu_index: usize) -> u16 {
    assert!(cpu_index < MAX_CPU_SLOTS);
    unsafe { TSS_SELECTORS[cpu_index] }
}
