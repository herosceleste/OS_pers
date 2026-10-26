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
        // K005-05 : une grande page découpée pointe vers une table ; ses bits
        // de cache porteraient sur la table, pas sur le MMIO.
        if (*pd).0[pd_index] & HUGE == 0 {
            return false;
        }
        (*pd).0[pd_index] |= WRITE_THROUGH | CACHE_DISABLE;
        asm!(
            "invlpg [{}]",
            in(reg) address,
            options(nostack, preserves_flags)
        );
    }

    true
}

// --- K005-02 et K005-05 : région utilisateur ------------------------------------
//
// Les pages utilisateur vivent dans PML4[1] (`ring3::USER_BASE`), sous des
// tables propres : le mappage identité du noyau, dans PML4[0], ne reçoit
// jamais le bit U/S. K005-05 : une frame mappée pour le ring 3 est retirée du
// mappage identité ; sa grande page de 2 Mio est d'abord découpée en pages de
// 4 Kio de mêmes attributs, dans une table allouée une fois pour toutes.

use core::sync::atomic::{AtomicBool, Ordering};

use elyon_core::ring3::{self, Ring3Error};
use elyon_core::user_pages::{self, PageKind, UserPageError};

static mut USER_PDPT: PageTable = PageTable([0; ENTRIES]);
static mut USER_PD: PageTable = PageTable([0; ENTRIES]);
static mut USER_PT: PageTable = PageTable([0; ENTRIES]);

/// Un seul CPU modifie les tables à la fois. Aucun détenteur n'attend un
/// autre CPU (les shootdowns ont lieu hors de ce verrou), et aucun
/// gestionnaire d'interruption ne le prend.
static PAGING_BUSY: AtomicBool = AtomicBool::new(false);

fn with_tables<T>(body: impl FnOnce() -> T) -> T {
    while PAGING_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let result = body();
    PAGING_BUSY.store(false, Ordering::Release);
    result
}

/// Index de `va` dans la table utilisateur : les 2 premiers Mio de la région.
fn user_index(va: u64) -> Result<usize, UserPageError> {
    let [pml4_index, pdpt_index, pd_index, pt_index] = ring3::indices(va);
    if va & 0xFFF != 0
        || !ring3::is_user_address(va)
        || ring3::indices(ring3::USER_BASE)[..3] != [pml4_index, pdpt_index, pd_index]
    {
        return Err(UserPageError::Frame);
    }
    Ok(pt_index)
}

/// Relie PML4[1] → PDPT → PD → PT, U/S et inscriptibles, jamais NX : les
/// droits fins sont ceux de chaque page. Idempotent.
fn link_user_tables() -> Result<(), Ring3Error> {
    let [pml4_index, pdpt_index, pd_index, _] = ring3::indices(ring3::USER_BASE);
    // SAFETY: tables statiques mappées en identité, sous PAGING_BUSY.
    unsafe {
        (*addr_of_mut!(USER_PD)).0[pd_index] =
            ring3::table_entry(addr_of_mut!(USER_PT) as u64, true, true)?;
        (*addr_of_mut!(USER_PDPT)).0[pdpt_index] =
            ring3::table_entry(addr_of_mut!(USER_PD) as u64, true, true)?;
        (*addr_of_mut!(PML4)).0[pml4_index] =
            ring3::table_entry(addr_of_mut!(USER_PDPT) as u64, true, true)?;
    }
    Ok(())
}

/// Mappe la page utilisateur `va` sur `frame`, droits de `kind`. L'entrée
/// doit être absente : aucun TLB ne peut alors en garder une traduction.
pub fn map_user_leaf(va: u64, frame: u64, kind: PageKind) -> Result<(), UserPageError> {
    let index = user_index(va)?;
    let entry = user_pages::user_leaf(frame, kind)?;
    with_tables(|| {
        link_user_tables().map_err(|_| UserPageError::Frame)?;
        // SAFETY: table statique, sous PAGING_BUSY.
        unsafe {
            let pt = addr_of_mut!(USER_PT);
            if (*pt).0[index] != 0 {
                return Err(UserPageError::Leaf);
            }
            (*pt).0[index] = entry;
        }
        Ok(())
    })?;
    check_user_page(va, frame, kind)
}

/// Retire la page utilisateur `va` et rend sa frame. Les autres CPU peuvent
/// en garder la traduction : l'appelant fait le shootdown avant de réutiliser
/// la frame.
pub fn unmap_user_leaf(va: u64) -> Result<u64, UserPageError> {
    let index = user_index(va)?;
    let frame = with_tables(|| {
        // SAFETY: table statique, sous PAGING_BUSY.
        unsafe {
            let pt = addr_of_mut!(USER_PT);
            let entry = (*pt).0[index];
            if entry & user_pages::PTE_PRESENT == 0 {
                return Err(UserPageError::NotPresent);
            }
            (*pt).0[index] = 0;
            Ok(entry & 0x000F_FFFF_FFFF_F000)
        }
    })?;
    crate::tlb::invalidate_local(va);
    Ok(frame)
}

/// Chemin complet vers `va` : U/S à chaque niveau, aucun niveau NX, feuille
/// exacte vers `frame` avec les droits de `kind`.
pub fn check_user_page(va: u64, frame: u64, kind: PageKind) -> Result<(), UserPageError> {
    let index = user_index(va)?;
    let [pml4_index, pdpt_index, pd_index, _] = ring3::indices(va);
    // SAFETY: lecture des tables statiques.
    unsafe {
        let upper = [
            (*addr_of!(PML4)).0[pml4_index],
            (*addr_of!(USER_PDPT)).0[pdpt_index],
            (*addr_of!(USER_PD)).0[pd_index],
        ];
        for entry in upper {
            if ring3::check_user_entry(entry, true).is_err() || entry & user_pages::PTE_NX != 0 {
                return Err(UserPageError::Leaf);
            }
        }
        user_pages::check_user_leaf((*addr_of!(USER_PT)).0[index], frame, kind)
    }
}

/// Entrée de la table utilisateur pour `va` (0 si absente).
pub fn user_entry(va: u64) -> Result<u64, UserPageError> {
    let index = user_index(va)?;
    // SAFETY: lecture d'une table statique.
    Ok(unsafe { (*addr_of!(USER_PT)).0[index] })
}

fn identity_pd(pdpt_index: usize) -> *mut PageTable {
    // SAFETY: `pdpt_index` < 4, borne du mappage identité.
    unsafe { core::ptr::addr_of_mut!((*addr_of_mut!(PDS))[pdpt_index]) }
}

/// Retire `phys` du mappage identité de ce CPU : découpe sa grande page si
/// besoin, puis rend son entrée absente. Les autres CPU peuvent garder la
/// traduction : l'appelant fait le shootdown avant de confier la frame au
/// ring 3. La table de découpe vient d'une allocation permanente.
pub fn remove_identity_alias(phys: u64) -> Result<(), UserPageError> {
    let (pdpt_index, pd_index, pt_index) = user_pages::identity_slot(phys, IDENTITY_LIMIT)?;
    with_tables(|| {
        // SAFETY: tables statiques et tables de découpe, mappées en identité,
        // sous PAGING_BUSY.
        unsafe {
            let pd = identity_pd(pdpt_index);
            let pde = (*pd).0[pd_index];
            if pde & HUGE != 0 {
                // Seule la RAM est confiée au ring 3 : une grande page aux
                // attributs de cache MMIO n'est jamais découpée ici.
                if pde & (WRITE_THROUGH | CACHE_DISABLE) != 0 {
                    return Err(UserPageError::Leaf);
                }
                let table = crate::memory::allocate_frame().ok_or(UserPageError::Frame)?;
                let small = table as *mut u64;
                for index in 0..ENTRIES {
                    small
                        .add(index)
                        .write(user_pages::small_entry(pde, index as u64)?);
                }
                // Même traduction, en pages de 4 Kio : la grande page encore
                // dans les TLB reste juste jusqu'au retrait ci-dessous.
                (*pd).0[pd_index] = table | PRESENT | WRITABLE;
            }
            let table = ((*pd).0[pd_index] & 0x000F_FFFF_FFFF_F000) as *mut u64;
            let entry = table.add(pt_index);
            if entry.read() & PRESENT == 0 {
                return Err(UserPageError::NotPresent);
            }
            entry.write(0);
        }
        Ok(())
    })?;
    crate::tlb::invalidate_local(phys);
    Ok(())
}

/// Rend `phys` au mappage identité, avec les attributs de la RAM. Une entrée
/// absente ne peut pas rester en TLB : aucun shootdown n'est nécessaire.
pub fn restore_identity_alias(phys: u64) -> Result<(), UserPageError> {
    let (pdpt_index, pd_index, pt_index) = user_pages::identity_slot(phys, IDENTITY_LIMIT)?;
    let huge = (phys & !(HUGE_PAGE_SIZE - 1)) | PRESENT | WRITABLE | HUGE;
    let restored = user_pages::small_entry(huge, pt_index as u64)?;
    with_tables(|| {
        // SAFETY: tables statiques et tables de découpe, sous PAGING_BUSY.
        unsafe {
            let pde = (*identity_pd(pdpt_index)).0[pd_index];
            if pde & HUGE != 0 {
                return Err(UserPageError::NotHuge);
            }
            let entry = ((pde & 0x000F_FFFF_FFFF_F000) as *mut u64).add(pt_index);
            if entry.read() != 0 {
                return Err(UserPageError::AliasPresent);
            }
            entry.write(restored);
        }
        Ok(())
    })
}

/// Entrée de `phys` dans le mappage identité : grande page ou page de 4 Kio,
/// 0 si l'alias a été retiré.
pub fn identity_entry(phys: u64) -> Result<u64, UserPageError> {
    let (pdpt_index, pd_index, pt_index) = user_pages::identity_slot(phys, IDENTITY_LIMIT)?;
    // SAFETY: lecture des tables statiques et des tables de découpe.
    unsafe {
        let pde = (*identity_pd(pdpt_index)).0[pd_index];
        if pde & HUGE != 0 {
            return Ok(pde);
        }
        Ok(((pde & 0x000F_FFFF_FFFF_F000) as *const u64)
            .add(pt_index)
            .read())
    }
}

/// Le mappage identité du noyau ne porte U/S à aucun niveau, tables de
/// découpe comprises.
pub fn check_kernel_identity() -> Result<usize, Ring3Error> {
    let mut split = 0;
    // SAFETY: lecture des tables statiques et des tables de découpe.
    unsafe {
        let pml4 = &*addr_of!(PML4);
        let kernel_pdpt = &*addr_of!(PDPT);
        let kernel_pds = &*addr_of!(PDS);
        ring3::check_kernel_entry(pml4.0[0])?;
        for (index, pd) in kernel_pds.iter().enumerate() {
            ring3::check_kernel_entry(kernel_pdpt.0[index])?;
            for &entry in &pd.0 {
                ring3::check_kernel_entry(entry)?;
                if entry & HUGE == 0 {
                    split += 1;
                    let table = (entry & 0x000F_FFFF_FFFF_F000) as *const u64;
                    for small in 0..ENTRIES {
                        if table.add(small).read() & ring3::PTE_USER != 0 {
                            return Err(Ring3Error::KernelUser);
                        }
                    }
                }
            }
        }
    }
    Ok(split)
}
