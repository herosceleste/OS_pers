use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, write_bytes};

const ENTRIES: usize = 512;
const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
const IDENTITY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const WRITE_THROUGH: u64 = 1 << 3;
const CACHE_DISABLE: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;

#[repr(C, align(4096))]
struct PageTable([u64; ENTRIES]);

static mut PML4: PageTable = PageTable([0; ENTRIES]);
static mut PDPT: PageTable = PageTable([0; ENTRIES]);
static mut PDS: [PageTable; 4] = [
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
];

pub fn install_identity_4g() -> u64 {
    unsafe {
        write_bytes(
            addr_of_mut!(PML4).cast::<u8>(),
            0,
            core::mem::size_of::<PageTable>(),
        );
        write_bytes(
            addr_of_mut!(PDPT).cast::<u8>(),
            0,
            core::mem::size_of::<PageTable>(),
        );
        write_bytes(
            addr_of_mut!(PDS).cast::<u8>(),
            0,
            core::mem::size_of::<[PageTable; 4]>(),
        );

        let pml4 = addr_of_mut!(PML4);
        let pdpt = addr_of_mut!(PDPT);
        let pds = addr_of_mut!(PDS);

        (*pml4).0[0] = pdpt as u64 | PRESENT | WRITABLE;

        for pdpt_index in 0..4usize {
            let pd = core::ptr::addr_of_mut!((*pds)[pdpt_index]);
            (*pdpt).0[pdpt_index] = pd as u64 | PRESENT | WRITABLE;

            for entry in 0..ENTRIES {
                let phys = ((pdpt_index * ENTRIES + entry) as u64) * HUGE_PAGE_SIZE;
                (*pd).0[entry] = phys | PRESENT | WRITABLE | HUGE;
            }
        }

        let pml4_phys = pml4 as u64;
        asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
        pml4_phys
    }
}

pub fn table_span() -> (u64, u64) {
    let pml4 = addr_of_mut!(PML4) as u64;
    let pdpt = addr_of_mut!(PDPT) as u64;
    let pds = addr_of_mut!(PDS) as u64;
    let pds_end = pds + core::mem::size_of::<[PageTable; 4]>() as u64;

    let start = pml4.min(pdpt).min(pds);
    let end = (pml4 + core::mem::size_of::<PageTable>() as u64)
        .max(pdpt + core::mem::size_of::<PageTable>() as u64)
        .max(pds_end);
    (start, end)
}

pub const fn identity_limit() -> u64 {
    IDENTITY_LIMIT
}

pub fn mark_identity_mmio_uncacheable(address: u64) -> bool {
    if address >= IDENTITY_LIMIT {
        return false;
    }

    let huge_index = (address / HUGE_PAGE_SIZE) as usize;
    let pdpt_index = huge_index / ENTRIES;
    let pd_index = huge_index % ENTRIES;

    if pdpt_index >= 4 {
        return false;
    }

    unsafe {
        let pds = addr_of_mut!(PDS);
        let pd = core::ptr::addr_of_mut!((*pds)[pdpt_index]);
        (*pd).0[pd_index] |= WRITE_THROUGH | CACHE_DISABLE;
        asm!(
            "invlpg [{}]",
            in(reg) address,
            options(nostack, preserves_flags)
        );
    }

    true
}

// --- K005-02 : région utilisateur minimale -----------------------------------------
//
// Les pages utilisateur vivent dans PML4[1] (`ring3::USER_BASE`), sous des
// tables propres : le mappage identité du noyau, dans PML4[0], ne reçoit
// jamais le bit U/S. La sonde ring 3 y mappe une page de code, en lecture
// seule, et une page de pile.

use elyon_core::ring3::{self, Ring3Error};

static mut USER_PDPT: PageTable = PageTable([0; ENTRIES]);
static mut USER_PD: PageTable = PageTable([0; ENTRIES]);
static mut USER_PT: PageTable = PageTable([0; ENTRIES]);

/// Mappe la page de code et la page de pile de la sonde. Appelé par le BSP
/// avant qu'aucun CPU n'utilise la région : aucune entrée n'était présente,
/// donc aucun TLB ne peut en garder une ancienne.
pub fn map_user_probe(code_frame: u64, stack_frame: u64) -> Result<(), Ring3Error> {
    let [pml4_index, pdpt_index, pd_index, code_index] = ring3::indices(ring3::USER_CODE_VA);
    let stack_index = ring3::indices(ring3::USER_STACK_VA)[3];
    // SAFETY: tables statiques mappées en identité, modifiées par le seul BSP
    // avant toute utilisation de la région utilisateur.
    unsafe {
        let pml4 = addr_of_mut!(PML4);
        let pdpt = addr_of_mut!(USER_PDPT);
        let pd = addr_of_mut!(USER_PD);
        let pt = addr_of_mut!(USER_PT);
        (*pt).0[code_index] = ring3::table_entry(code_frame, true, false)?;
        (*pt).0[stack_index] = ring3::table_entry(stack_frame, true, true)?;
        (*pd).0[pd_index] = ring3::table_entry(pt as u64, true, true)?;
        (*pdpt).0[pdpt_index] = ring3::table_entry(pd as u64, true, true)?;
        (*pml4).0[pml4_index] = ring3::table_entry(pdpt as u64, true, true)?;
    }
    Ok(())
}

/// Relit les deux chemins : la région utilisateur porte le bit U/S à chaque
/// niveau, le mappage identité du noyau ne le porte à aucun.
pub fn check_user_probe() -> Result<(), Ring3Error> {
    let [pml4_index, pdpt_index, pd_index, code_index] = ring3::indices(ring3::USER_CODE_VA);
    let stack_index = ring3::indices(ring3::USER_STACK_VA)[3];
    // SAFETY: lecture des tables statiques.
    unsafe {
        let pml4 = &*addr_of!(PML4);
        let pdpt = &*addr_of!(USER_PDPT);
        let pd = &*addr_of!(USER_PD);
        let pt = &*addr_of!(USER_PT);
        ring3::check_user_entry(pml4.0[pml4_index], true)?;
        ring3::check_user_entry(pdpt.0[pdpt_index], true)?;
        ring3::check_user_entry(pd.0[pd_index], true)?;
        ring3::check_user_entry(pt.0[code_index], false)?;
        if pt.0[code_index] & ring3::PTE_WRITABLE != 0 {
            return Err(Ring3Error::NotWritable);
        }
        ring3::check_user_entry(pt.0[stack_index], true)?;

        let kernel_pdpt = &*addr_of!(PDPT);
        let kernel_pds = &*addr_of!(PDS);
        ring3::check_kernel_entry(pml4.0[0])?;
        for (index, pd) in kernel_pds.iter().enumerate() {
            ring3::check_kernel_entry(kernel_pdpt.0[index])?;
            for &entry in &pd.0 {
                ring3::check_kernel_entry(entry)?;
            }
        }
    }
    Ok(())
}
