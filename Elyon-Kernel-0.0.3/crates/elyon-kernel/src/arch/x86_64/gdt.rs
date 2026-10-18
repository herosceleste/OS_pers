use core::ptr::{addr_of, addr_of_mut};

use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::{PrivilegeLevel, VirtAddr};

use super::MAX_CPU_SLOTS;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct ExceptionStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut BSP_DOUBLE_FAULT_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);
static mut TSS_TABLE: [TaskStateSegment; MAX_CPU_SLOTS] =
    [const { TaskStateSegment::new() }; MAX_CPU_SLOTS];
static mut GDT_TABLE: [GlobalDescriptorTable<8>; MAX_CPU_SLOTS] =
    [const { GlobalDescriptorTable::<8>::empty() }; MAX_CPU_SLOTS];
static mut CODE_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut DATA_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut TSS_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut IST_STARTS: [u64; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut IST_ENDS: [u64; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];

pub fn init() -> u16 {
    let (ist_start, ist_end) = bsp_double_fault_stack_span();
    prepare_for_cpu(0, ist_start, ist_end);
    load_for_cpu(0)
}

pub fn prepare_for_cpu(cpu_index: usize, ist_start: u64, ist_end: u64) {
    assert!(cpu_index < MAX_CPU_SLOTS);
    assert!(ist_start != 0 && ist_end > ist_start);

    unsafe {
        IST_STARTS[cpu_index] = ist_start;
        IST_ENDS[cpu_index] = ist_end;

        let tss_table = addr_of_mut!(TSS_TABLE);
        (*tss_table)[cpu_index] = TaskStateSegment::new();
        (*tss_table)[cpu_index].interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            VirtAddr::new(ist_end);

        let gdt_table = addr_of_mut!(GDT_TABLE);
        (*gdt_table)[cpu_index] = GlobalDescriptorTable::<8>::empty();

        let code = (*gdt_table)[cpu_index].append(Descriptor::kernel_code_segment());
        let data = (*gdt_table)[cpu_index].append(Descriptor::kernel_data_segment());
        let tss_ptr = addr_of!((*tss_table)[cpu_index]);
        let tss_descriptor = Descriptor::tss_segment_unchecked(tss_ptr);
        let tss = (*gdt_table)[cpu_index].append(tss_descriptor);

        CODE_SELECTORS[cpu_index] = code.index() << 3;
        DATA_SELECTORS[cpu_index] = data.index() << 3;
        TSS_SELECTORS[cpu_index] = tss.index() << 3;
    }
}

pub fn load_for_cpu(cpu_index: usize) -> u16 {
    assert!(cpu_index < MAX_CPU_SLOTS);

    unsafe {
        let gdt_table = addr_of_mut!(GDT_TABLE);
        let code = SegmentSelector::new(CODE_SELECTORS[cpu_index] >> 3, PrivilegeLevel::Ring0);
        let data = SegmentSelector::new(DATA_SELECTORS[cpu_index] >> 3, PrivilegeLevel::Ring0);
        let tss = SegmentSelector::new(TSS_SELECTORS[cpu_index] >> 3, PrivilegeLevel::Ring0);

        (*gdt_table)[cpu_index].load_unsafe();
        CS::set_reg(code);
        DS::set_reg(data);
        ES::set_reg(data);
        SS::set_reg(data);
        load_tss(tss);

        CODE_SELECTORS[cpu_index]
    }
}

fn bsp_double_fault_stack_span() -> (u64, u64) {
    let start = addr_of!(BSP_DOUBLE_FAULT_STACK) as u64;
    (start, start + DOUBLE_FAULT_STACK_SIZE as u64)
}

pub fn double_fault_stack_span() -> (u64, u64) {
    double_fault_stack_span_for(0)
}

pub fn double_fault_stack_span_for(cpu_index: usize) -> (u64, u64) {
    assert!(cpu_index < MAX_CPU_SLOTS);
    unsafe { (IST_STARTS[cpu_index], IST_ENDS[cpu_index]) }
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
