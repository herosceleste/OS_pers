use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use elyon_core::segments::{self, LAYOUT};
use x86_64::instructions::tables::{load_tss, sgdt};
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::{PrivilegeLevel, VirtAddr};

use super::MAX_CPU_SLOTS;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
/// Entrées de la GDT : null, code et données noyau, données et code
/// utilisateur, TSS sur deux entrées (K005-01, disposition `segments::LAYOUT`).
pub const GDT_ENTRIES: usize = 7;

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
        // K005-01 : données puis code utilisateur, l'ordre qu'impose `sysret`.
        let user_data = (*gdt_table)[cpu_index].append(Descriptor::user_data_segment());
        let user_code = (*gdt_table)[cpu_index].append(Descriptor::user_code_segment());
        let tss_ptr = addr_of!((*tss_table)[cpu_index]);
        let tss_descriptor = Descriptor::tss_segment_unchecked(tss_ptr);
        let tss = (*gdt_table)[cpu_index].append(tss_descriptor);
        assert!(
            code.index() << 3 == LAYOUT.kernel_code
                && data.index() << 3 == LAYOUT.kernel_data
                && user_data.index() << 3 == LAYOUT.user_data
                && user_code.index() << 3 == LAYOUT.user_code
                && tss.index() << 3 == LAYOUT.tss
        );

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

/// Pile noyau `RSP0` du TSS de `cpu_index` : celle que prend toute entrée
/// depuis le ring 3 (K005-01). Écrite par ce seul CPU, IRQ masquées, avant le
/// switch vers la tâche qui la possède ; lue aussi par le processeur, d'où
/// l'écriture volatile.
pub fn set_rsp0(cpu_index: usize, rsp0: u64) {
    let halves = rsp0_halves(cpu_index);
    // SAFETY: le TSS de ce CPU n'est écrit que par lui-même, IRQ masquées ;
    // les deux moitiés sont des u32 alignés sur 4 (voir `rsp0_halves`).
    unsafe {
        write_volatile(halves, rsp0 as u32);
        write_volatile(halves.add(1), (rsp0 >> 32) as u32);
    }
}

pub fn rsp0(cpu_index: usize) -> u64 {
    let halves = rsp0_halves(cpu_index);
    // SAFETY: lecture de deux u32 alignés du TSS statique de ce CPU.
    unsafe { u64::from(read_volatile(halves)) | (u64::from(read_volatile(halves.add(1))) << 32) }
}

/// RSP0 vu comme deux u32. Le TSS est `packed(4)` : RSP0, à l'offset 4, n'est
/// aligné que sur 4 octets, un accès volatile de 8 octets serait mal aligné.
fn rsp0_halves(cpu_index: usize) -> *mut u32 {
    assert!(cpu_index < MAX_CPU_SLOTS);
    // SAFETY: calcul d'adresse dans le TSS statique de ce CPU, sans accès.
    unsafe {
        let table = addr_of_mut!(TSS_TABLE);
        addr_of_mut!((*table)[cpu_index].privilege_stack_table[0]).cast::<u32>()
    }
}

/// Base du bitmap d'E/S du TSS de `cpu_index`.
pub fn iomap_base(cpu_index: usize) -> u16 {
    assert!(cpu_index < MAX_CPU_SLOTS);
    // SAFETY: lecture d'un u16 du TSS statique ; `addr_of!` évite toute
    // référence vers un champ d'une structure `packed`.
    unsafe {
        let table = addr_of!(TSS_TABLE);
        read_volatile(addr_of!((*table)[cpu_index].iomap_base))
    }
}

/// Entrées de la GDT chargée sur ce CPU, relues via `sgdt` : la preuve porte
/// sur la table que le processeur utilise, pas sur la copie préparée.
pub fn loaded_entries() -> Option<[u64; GDT_ENTRIES]> {
    let pointer = sgdt();
    if usize::from(pointer.limit) + 1 < GDT_ENTRIES * 8 {
        return None;
    }
    let base = pointer.base.as_u64() as *const u64;
    // SAFETY: `sgdt` rend la base et la limite de la GDT chargée, une table
    // statique du noyau mappée en identité ; la limite couvre GDT_ENTRIES.
    Some(core::array::from_fn(|index| unsafe {
        read_volatile(base.add(index))
    }))
}

/// Contrôle de la GDT chargée sur ce CPU ; rend la valeur `STAR` qu'elle permet.
pub fn check_loaded(cpu_index: usize) -> Result<u64, segments::SegmentError> {
    let entries = loaded_entries().ok_or(segments::SegmentError::TooShort)?;
    segments::check_gdt(&entries, &LAYOUT)?;
    segments::check_iomap(iomap_base(cpu_index), segments::TSS_LIMIT)?;
    segments::star(&LAYOUT)
}
