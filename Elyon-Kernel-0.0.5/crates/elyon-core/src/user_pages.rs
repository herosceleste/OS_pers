//! Pages utilisateur (K005-05), sans état global.
//!
//! Une page utilisateur est une frame prise par `allocate_owned`, mappée dans
//! la région utilisateur avec des droits exacts : code en lecture seule et
//! exécutable, données inscriptibles et jamais exécutables (NX). Le mappage
//! identité du noyau couvre toute la mémoire physique en grandes pages de
//! 2 Mio : tant qu'elle sert au ring 3, la frame en est retirée. La grande
//! page est découpée en pages de 4 Kio de mêmes attributs, l'entrée de la
//! frame est rendue absente et la traduction invalidée sur tous les CPU (TLB
//! shootdown) : un accès du noyau par cet alias lève une #PF. À la libération,
//! le mappage utilisateur est retiré et invalidé partout avant que l'alias ne
//! revienne et que la frame ne retourne au PMM.

pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
const PTE_PWT: u64 = 1 << 3;
const PTE_PCD: u64 = 1 << 4;
const PTE_ACCESSED: u64 = 1 << 5;
const PTE_DIRTY: u64 = 1 << 6;
/// PS dans une PDE ; PAT dans une PTE de 4 Kio.
const PDE_HUGE: u64 = 1 << 7;
const PTE_PAT_4K: u64 = 1 << 7;
pub const PTE_GLOBAL: u64 = 1 << 8;
const PDE_PAT_2M: u64 = 1 << 12;
pub const PTE_NX: u64 = 1 << 63;
const FRAME_4K: u64 = 0x000F_FFFF_FFFF_F000;
const FRAME_2M: u64 = 0x000F_FFFF_FFE0_0000;

const PAGE_SIZE: u64 = 0x1000;
const ENTRIES: u64 = 512;
const HUGE_PAGE_SIZE: u64 = PAGE_SIZE * ENTRIES;
const GIB: u64 = HUGE_PAGE_SIZE * ENTRIES;

/// CPUID.80000001H:EDX.NX[20] et IA32_EFER.NXE[11].
pub const CPUID_EXT1_EDX_NX: u32 = 1 << 20;
const CPUID_EXT_LEAF_1: u32 = 0x8000_0001;
pub const EFER_NXE: u64 = 1 << 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// Lecture seule, exécutable.
    Code,
    /// Lecture et écriture, jamais exécutable.
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserPageError {
    /// Frame non alignée sur 4 Kio, au-delà de 52 bits, ou index hors table.
    Frame,
    /// Entrée sans les droits exacts attendus.
    Leaf,
    NotPresent,
    /// L'entrée du mappage identité n'est pas une grande page.
    NotHuge,
    /// Frame au-delà du mappage identité.
    OutsideIdentity,
    /// L'alias identité d'une page utilisateur est encore présent.
    AliasPresent,
    NxUnsupported,
    NxDisabled,
}

impl UserPageError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "page frame is misaligned, beyond 52 bits or out of range",
            Self::Leaf => "page table entry does not carry the exact expected rights",
            Self::NotPresent => "page table entry is not present",
            Self::NotHuge => "identity map entry is not a 2 MiB page",
            Self::OutsideIdentity => "frame is beyond the identity map",
            Self::AliasPresent => "user frame is still reachable through the identity map",
            Self::NxUnsupported => "CPU does not support NX",
            Self::NxDisabled => "EFER.NXE is clear",
        }
    }
}

/// Entrée de la page utilisateur `frame` : U/S, présente, droits de `kind`.
pub const fn user_leaf(frame: u64, kind: PageKind) -> Result<u64, UserPageError> {
    if frame & !FRAME_4K != 0 {
        return Err(UserPageError::Frame);
    }
    let rights = match kind {
        PageKind::Code => PTE_PRESENT | PTE_USER,
        PageKind::Data => PTE_PRESENT | PTE_USER | PTE_WRITABLE | PTE_NX,
    };
    Ok(frame | rights)
}

/// Entrée relue contre `user_leaf` ; seuls Accessed et Dirty, posés par le
/// processeur, peuvent s'y ajouter.
pub const fn check_user_leaf(entry: u64, frame: u64, kind: PageKind) -> Result<(), UserPageError> {
    match user_leaf(frame, kind) {
        Ok(expected) if entry & !(PTE_ACCESSED | PTE_DIRTY) == expected => Ok(()),
        _ => Err(UserPageError::Leaf),
    }
}

pub const fn nx_supported(max_extended_leaf: u32, ext1_edx: u32) -> bool {
    max_extended_leaf >= CPUID_EXT_LEAF_1 && ext1_edx & CPUID_EXT1_EDX_NX != 0
}

pub const fn check_efer(efer: u64) -> Result<(), UserPageError> {
    if efer & EFER_NXE == 0 {
        return Err(UserPageError::NxDisabled);
    }
    Ok(())
}

/// Indices PDPT, PD et PT de `phys` dans le mappage identité `[0, limit)`.
pub const fn identity_slot(phys: u64, limit: u64) -> Result<(usize, usize, usize), UserPageError> {
    if phys & (PAGE_SIZE - 1) != 0 {
        return Err(UserPageError::Frame);
    }
    if phys >= limit {
        return Err(UserPageError::OutsideIdentity);
    }
    Ok((
        (phys / GIB) as usize,
        ((phys / HUGE_PAGE_SIZE) % ENTRIES) as usize,
        ((phys / PAGE_SIZE) % ENTRIES) as usize,
    ))
}

/// Entrée `index` de la table de 4 Kio qui remplace la grande page `pde` :
/// même traduction et mêmes attributs (cache, PAT déplacé du bit 12 au bit 7,
/// NX). Une PDE du mappage identité ne porte jamais U/S.
pub const fn small_entry(pde: u64, index: u64) -> Result<u64, UserPageError> {
    if pde & PTE_PRESENT == 0 {
        return Err(UserPageError::NotPresent);
    }
    if pde & PDE_HUGE == 0 {
        return Err(UserPageError::NotHuge);
    }
    if pde & PTE_USER != 0 {
        return Err(UserPageError::Leaf);
    }
    if index >= ENTRIES {
        return Err(UserPageError::Frame);
    }
    let mut flags = pde & (PTE_PRESENT | PTE_WRITABLE | PTE_PWT | PTE_PCD | PTE_GLOBAL | PTE_NX);
    if pde & PDE_PAT_2M != 0 {
        flags |= PTE_PAT_4K;
    }
    Ok(((pde & FRAME_2M) + index * PAGE_SIZE) | flags)
}

/// CPU à invalider : tous les CPU en ligne sauf l'émetteur.
pub const fn shootdown_targets(online: usize, sender: usize) -> u64 {
    let all = if online >= 64 {
        u64::MAX
    } else {
        (1u64 << online) - 1
    };
    all & !(1u64 << sender)
}

/// Vrai quand chaque CPU de `targets` a acquitté la génération `generation`.
/// Une cible sans case d'acquittement n'est jamais tenue pour acquittée.
pub fn all_acked(acks: &[u64], targets: u64, generation: u64) -> bool {
    if acks.len() < 64 && targets >> acks.len() != 0 {
        return false;
    }
    acks.iter()
        .enumerate()
        .all(|(cpu, &ack)| cpu >= 64 || targets & (1 << cpu) == 0 || ack >= generation)
}
