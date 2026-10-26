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
const ACCESSED_DIRTY: u64 = (1 << 5) | (1 << 6);

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

use core::sync::atomic::{AtomicBool, Ordering, compiler_fence};

use elyon_core::ring3::{self, Ring3Error};
use elyon_core::user_pages::{self, PageKind, UserPageError};

static mut USER_PDPT: PageTable = PageTable([0; ENTRIES]);
static mut USER_PD: PageTable = PageTable([0; ENTRIES]);
static mut USER_PT: PageTable = PageTable([0; ENTRIES]);

/// Un seul CPU modifie les tables à la fois, IRQ masquées : le détenteur ne
/// peut être ni préempté ni interrompu. Aucun détenteur n'attend un autre CPU
/// (les shootdowns ont lieu hors de ce verrou) ; il peut prendre PMM_LOCK
/// (table de découpe), jamais l'inverse : le PMM n'appelle pas `paging`.
static PAGING_BUSY: AtomicBool = AtomicBool::new(false);

fn with_tables<T>(body: impl FnOnce() -> T) -> T {
    let interrupts = super::cpu::interrupts_enabled();
    super::cpu::disable_interrupts();
    while PAGING_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let result = body();
    PAGING_BUSY.store(false, Ordering::Release);
    if interrupts {
        super::cpu::enable_interrupts();
    }
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
/// doit être absente. Les niveaux supérieurs sont reliés et contrôlés avant
/// l'écriture de la feuille, relue ensuite ; si la relecture échoue, la
/// feuille est retirée avant de rendre l'erreur, et l'appelant fait le
/// shootdown avant de rendre la frame.
pub fn map_user_leaf(va: u64, frame: u64, kind: PageKind) -> Result<(), UserPageError> {
    let index = user_index(va)?;
    let entry = user_pages::user_leaf(frame, kind)?;
    let result = with_tables(|| {
        link_user_tables().map_err(|_| UserPageError::Frame)?;
        check_upper_levels(va)?;
        // SAFETY: table statique, sous PAGING_BUSY ; `index` < 512 vient de
        // `user_index`. Accès volatils : la table est lue par la MMU.
        unsafe {
            let leaf = core::ptr::addr_of_mut!((*addr_of_mut!(USER_PT)).0[index]);
            if leaf.read_volatile() != 0 {
                return Err(UserPageError::Leaf);
            }
            leaf.write_volatile(entry);
            if let Err(error) = user_pages::check_user_leaf(leaf.read_volatile(), frame, kind) {
                leaf.write_volatile(0);
                return Err(error);
            }
        }
        Ok(())
    });
    if result.is_err() {
        crate::tlb::invalidate_local(va);
    }
    result
}

/// Retire la page utilisateur `va`, qui doit mapper `frame` : l'entrée n'est
/// effacée qu'après cette vérification. Les autres CPU peuvent en garder la
/// traduction : l'appelant fait le shootdown avant de rendre la frame.
pub fn unmap_user_leaf(va: u64, frame: u64) -> Result<(), UserPageError> {
    let index = user_index(va)?;
    with_tables(|| {
        // SAFETY: table statique, sous PAGING_BUSY ; `index` < 512.
        unsafe {
            let leaf = core::ptr::addr_of_mut!((*addr_of_mut!(USER_PT)).0[index]);
            let entry = leaf.read_volatile();
            if entry & user_pages::PTE_PRESENT == 0 {
                return Err(UserPageError::NotPresent);
            }
            if entry & user_pages::PTE_FRAME != frame {
                return Err(UserPageError::Leaf);
            }
            leaf.write_volatile(0);
        }
        Ok(())
    })?;
    crate::tlb::invalidate_local(va);
    Ok(())
}

/// PML4, PDPT et PD du chemin vers `va` : U/S et inscriptibles, jamais NX.
fn check_upper_levels(va: u64) -> Result<(), UserPageError> {
    let [pml4_index, pdpt_index, pd_index, _] = ring3::indices(va);
    // SAFETY: lecture des tables statiques.
    let upper = unsafe {
        [
            (*addr_of!(PML4)).0[pml4_index],
            (*addr_of!(USER_PDPT)).0[pdpt_index],
            (*addr_of!(USER_PD)).0[pd_index],
        ]
    };
    for entry in upper {
        if ring3::check_user_entry(entry, true).is_err() || entry & user_pages::PTE_NX != 0 {
            return Err(UserPageError::Leaf);
        }
    }
    Ok(())
}

/// Chemin complet vers `va` : U/S à chaque niveau, aucun niveau NX, feuille
/// exacte vers `frame` avec les droits de `kind`. Lecture sans PAGING_BUSY :
/// les appelants (qualification, `UserPage`) ne modifient pas la page en
/// même temps.
pub fn check_user_page(va: u64, frame: u64, kind: PageKind) -> Result<(), UserPageError> {
    let index = user_index(va)?;
    check_upper_levels(va)?;
    // SAFETY: lecture d'une table statique ; `index` < 512.
    let leaf = unsafe { core::ptr::addr_of!((*addr_of!(USER_PT)).0[index]).read_volatile() };
    user_pages::check_user_leaf(leaf, frame, kind)
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
        // SAFETY: `pdpt_index` < 4 et `pd_index`, `pt_index` < 512 viennent de
        // `identity_slot` ; la PD est statique, la table de découpe une frame
        // permanente sous la limite de l'identité, mappée en identité. Tout
        // sous PAGING_BUSY, accès volatils : les tables sont lues par la MMU.
        unsafe {
            let pde_ptr = core::ptr::addr_of_mut!((*identity_pd(pdpt_index)).0[pd_index]);
            let pde = pde_ptr.read_volatile();
            if pde & PRESENT == 0 {
                return Err(UserPageError::NotPresent);
            }
            if pde & HUGE != 0 {
                // Seule la RAM est confiée au ring 3 : une grande page aux
                // attributs de cache MMIO n'est jamais découpée ici. La PDE
                // est validée avant toute allocation.
                if pde & (WRITE_THROUGH | CACHE_DISABLE) != 0 {
                    return Err(UserPageError::Leaf);
                }
                user_pages::small_entry(pde, 0)?;
                let table = crate::memory::allocate_frame().ok_or(UserPageError::OutOfMemory)?;
                let small = table as *mut u64;
                for index in 0..ENTRIES {
                    small
                        .add(index)
                        .write_volatile(user_pages::small_entry(pde, index as u64)?);
                }
                // La table est complète avant que la PDE ne la désigne. Même
                // traduction, en pages de 4 Kio : la grande page encore dans
                // les TLB reste juste jusqu'au retrait ci-dessous.
                compiler_fence(Ordering::Release);
                pde_ptr.write_volatile(table | PRESENT | WRITABLE);
            }
            let table = (pde_ptr.read_volatile() & user_pages::PTE_FRAME) as *mut u64;
            let entry = table.add(pt_index);
            if entry.read_volatile() & PRESENT == 0 {
                return Err(UserPageError::NotPresent);
            }
            entry.write_volatile(0);
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
        // SAFETY: indices de `identity_slot` ; PDE présente non HUGE, donc
        // une table de découpe permanente mappée en identité ; sous
        // PAGING_BUSY.
        unsafe {
            let pde = core::ptr::addr_of!((*identity_pd(pdpt_index)).0[pd_index]).read_volatile();
            if pde & PRESENT == 0 || pde & HUGE != 0 {
                return Err(UserPageError::NotHuge);
            }
            let entry = ((pde & user_pages::PTE_FRAME) as *mut u64).add(pt_index);
            if entry.read_volatile() != 0 {
                return Err(UserPageError::AliasPresent);
            }
            entry.write_volatile(restored);
        }
        Ok(())
    })
}

/// Entrée de `phys` dans le mappage identité : grande page ou page de 4 Kio,
/// 0 si l'alias a été retiré.
pub fn identity_entry(phys: u64) -> Result<u64, UserPageError> {
    let (pdpt_index, pd_index, pt_index) = user_pages::identity_slot(phys, IDENTITY_LIMIT)?;
    // SAFETY: indices de `identity_slot` ; une PDE présente non HUGE désigne
    // une table de découpe permanente mappée en identité. Lecture sans
    // PAGING_BUSY : les appelants ne modifient pas cette frame en même temps.
    unsafe {
        let pde = core::ptr::addr_of!((*identity_pd(pdpt_index)).0[pd_index]).read_volatile();
        if pde & PRESENT == 0 {
            return Ok(0);
        }
        if pde & HUGE != 0 {
            return Ok(pde);
        }
        Ok(((pde & user_pages::PTE_FRAME) as *const u64)
            .add(pt_index)
            .read_volatile())
    }
}

/// Le mappage identité du noyau ne porte U/S à aucun niveau. Chaque entrée
/// d'une table de découpe est soit absente (alias retiré), soit exactement la
/// page de 4 Kio de RAM de sa propre adresse, aux bits Accessed et Dirty près. Rend le nombre de tables de
/// découpe.
pub fn check_kernel_identity() -> Result<usize, Ring3Error> {
    let mut split = 0;
    // SAFETY: lecture des tables statiques ; une PDE présente non HUGE
    // désigne une table de découpe permanente mappée en identité.
    unsafe {
        let pml4 = &*addr_of!(PML4);
        let kernel_pdpt = &*addr_of!(PDPT);
        let kernel_pds = &*addr_of!(PDS);
        ring3::check_kernel_entry(pml4.0[0])?;
        for (pdpt_index, pd) in kernel_pds.iter().enumerate() {
            ring3::check_kernel_entry(kernel_pdpt.0[pdpt_index])?;
            for (pd_index, &entry) in pd.0.iter().enumerate() {
                ring3::check_kernel_entry(entry)?;
                if entry & HUGE != 0 {
                    continue;
                }
                split += 1;
                let base = ((pdpt_index * ENTRIES + pd_index) as u64) * HUGE_PAGE_SIZE;
                let huge = base | PRESENT | WRITABLE | HUGE;
                let table = (entry & user_pages::PTE_FRAME) as *const u64;
                for small in 0..ENTRIES {
                    // Accessed et Dirty sont posés par le processeur.
                    let value = table.add(small).read_volatile() & !ACCESSED_DIRTY;
                    if value != 0 && Ok(value) != user_pages::small_entry(huge, small as u64) {
                        return Err(Ring3Error::KernelUser);
                    }
                }
            }
        }
    }
    Ok(split)
}
