use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use elyon_core::paranoid;
use elyon_core::segments::{self, LAYOUT};
use x86_64::instructions::tables::{load_tss, sgdt};
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::{PrivilegeLevel, VirtAddr};

use super::{MAX_CPU_SLOTS, cpu_local};

/// Index dans `interrupt_stack_table` du TSS : numéro d'IST de la porte - 1.
pub const DOUBLE_FAULT_IST_INDEX: u16 = paranoid::IST_DOUBLE_FAULT - 1;
pub const NMI_IST_INDEX: u16 = paranoid::IST_NMI - 1;
pub const MACHINE_CHECK_IST_INDEX: u16 = paranoid::IST_MACHINE_CHECK - 1;
/// Piles IST paranoïaques par CPU : #DF, NMI, #MC (K005-03).
pub const PARANOID_ISTS: usize = 3;
pub const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
const _: () = assert!(DOUBLE_FAULT_STACK_SIZE as u64 >= paranoid::MIN_IST_SIZE);
/// Entrées de la GDT : null, code et données noyau, données et code
/// utilisateur, TSS sur deux entrées (K005-01, disposition `segments::LAYOUT`).
pub const GDT_ENTRIES: usize = 7;

#[repr(C, align(16))]
struct ExceptionStack([u8; DOUBLE_FAULT_STACK_SIZE]);

static mut BSP_DOUBLE_FAULT_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);
static mut BSP_NMI_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);
static mut BSP_MACHINE_CHECK_STACK: ExceptionStack = ExceptionStack([0; DOUBLE_FAULT_STACK_SIZE]);
static mut TSS_TABLE: [TaskStateSegment; MAX_CPU_SLOTS] =
    [const { TaskStateSegment::new() }; MAX_CPU_SLOTS];
static mut GDT_TABLE: [GlobalDescriptorTable<8>; MAX_CPU_SLOTS] =
    [const { GlobalDescriptorTable::<8>::empty() }; MAX_CPU_SLOTS];
static mut CODE_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut DATA_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
static mut TSS_SELECTORS: [u16; MAX_CPU_SLOTS] = [0; MAX_CPU_SLOTS];
/// Piles `[start, end)` d'un CPU, rangées par index IST du TSS.
static mut IST_SPANS: [[(u64, u64); PARANOID_ISTS]; MAX_CPU_SLOTS] =
    [[(0, 0); PARANOID_ISTS]; MAX_CPU_SLOTS];

/// Piles IST paranoïaques d'un CPU (K005-03).
#[derive(Clone, Copy)]
pub struct IstSpans {
    pub double_fault: (u64, u64),
    pub nmi: (u64, u64),
    pub machine_check: (u64, u64),
}

pub fn init() -> u16 {
    prepare_for_cpu(
        0,
        IstSpans {
            double_fault: static_span(addr_of!(BSP_DOUBLE_FAULT_STACK)),
            nmi: static_span(addr_of!(BSP_NMI_STACK)),
            machine_check: static_span(addr_of!(BSP_MACHINE_CHECK_STACK)),
        },
    );
    load_for_cpu(0)
}

pub fn prepare_for_cpu(cpu_index: usize, spans: IstSpans) {
    assert!(cpu_index < MAX_CPU_SLOTS);
    let mut ordered = [(0, 0); PARANOID_ISTS];
    ordered[DOUBLE_FAULT_IST_INDEX as usize] = spans.double_fault;
    ordered[NMI_IST_INDEX as usize] = spans.nmi;
    ordered[MACHINE_CHECK_IST_INDEX as usize] = spans.machine_check;
    assert!(paranoid::check_spans(&ordered).is_ok());

    unsafe {
        let tss_table = addr_of_mut!(TSS_TABLE);
        (*tss_table)[cpu_index] = TaskStateSegment::new();
        for (index, &(start, end)) in ordered.iter().enumerate() {
            let pointer = match paranoid::ist_pointer(start, end) {
                Ok(pointer) => pointer,
                Err(_) => panic!("invalid IST stack"),
            };
            // K005-03 : le stub paranoïaque relit ici le GS_BASE noyau de ce
            // CPU, sans passer par GS ni se fier au CS sauvegardé.
            let slot = pointer as *mut u64;
            write_volatile(slot, cpu_local::area_address(cpu_index));
            write_volatile(slot.add(1), 0);
            (*tss_table)[cpu_index].interrupt_stack_table[index] = VirtAddr::new(pointer);
        }
        IST_SPANS[cpu_index] = ordered;

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

fn static_span(stack: *const ExceptionStack) -> (u64, u64) {
    let start = stack as u64;
    (start, start + DOUBLE_FAULT_STACK_SIZE as u64)
}

pub fn double_fault_stack_span_for(cpu_index: usize) -> (u64, u64) {
    ist_span_for(cpu_index, DOUBLE_FAULT_IST_INDEX)
}

/// Pile `[start, end)` de l'IST `index` (index du TSS) de `cpu_index`.
pub fn ist_span_for(cpu_index: usize, index: u16) -> (u64, u64) {
    assert!(cpu_index < MAX_CPU_SLOTS && usize::from(index) < PARANOID_ISTS);
    unsafe { IST_SPANS[cpu_index][usize::from(index)] }
}

/// Pointeur IST `index` relu dans le TSS de `cpu_index`, et le mot GS noyau
/// qui y est rangé : ce que le processeur et le stub utiliseront vraiment.
pub fn ist_pointer_and_slot(cpu_index: usize, index: u16) -> (u64, u64) {
    assert!(cpu_index < MAX_CPU_SLOTS && usize::from(index) < PARANOID_ISTS);
    // SAFETY: lecture de deux u32 alignés sur 4 du TSS statique (`packed(4)`,
    // comme RSP0), puis du mot posé par `prepare_for_cpu` sous ce pointeur,
    // dans une pile IST mappée en identité.
    unsafe {
        let table = addr_of!(TSS_TABLE);
        let halves =
            addr_of!((*table)[cpu_index].interrupt_stack_table[usize::from(index)]).cast::<u32>();
        let pointer =
            u64::from(read_volatile(halves)) | (u64::from(read_volatile(halves.add(1))) << 32);
        let slot = if pointer == 0 {
            0
        } else {
            read_volatile(pointer as *const u64)
        };
        (pointer, slot)
    }
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
