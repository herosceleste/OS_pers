//! Frontière ring 3 minimale (K005-02), sans état global.
//!
//! Une entrée d'interruption vient du ring 3 quand le RPL du CS sauvegardé
//! vaut 3 : c'est le seul critère que les stubs d'entrée consultent pour
//! décider d'un `swapgs`. Les pages utilisateur vivent dans leur propre
//! entrée PML4 (index 1, à 512 Gio) : le mappage identité du noyau, dans
//! PML4[0], reste entièrement superviseur. Ce module donne la région
//! utilisateur, les entrées de table de pages et le cadre `iretq` qui fait
//! entrer une tâche en ring 3.

use crate::segments::{self, LAYOUT};

/// Début de la région utilisateur : PML4[1].
pub const USER_BASE: u64 = 0x0000_0080_0000_0000;
/// Fin exclue de la moitié basse canonique.
pub const USER_LIMIT: u64 = 0x0000_8000_0000_0000;
pub const PAGE_SIZE: u64 = 0x1000;
/// Sonde ring 3 : une page de code, puis une page de pile.
pub const USER_CODE_VA: u64 = USER_BASE;
pub const USER_STACK_VA: u64 = USER_BASE + PAGE_SIZE;
pub const USER_STACK_TOP: u64 = USER_STACK_VA + PAGE_SIZE;

pub const PTE_PRESENT: u64 = 1 << 0;
pub const PTE_WRITABLE: u64 = 1 << 1;
pub const PTE_USER: u64 = 1 << 2;
/// Bits 51:12 : adresse physique d'une page ou d'une table.
const PTE_FRAME: u64 = 0x000F_FFFF_FFFF_F000;

/// RFLAGS d'entrée en ring 3 : IF, bit 1 réservé, IOPL 0.
const USER_RFLAGS: u64 = 0x202;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring3Error {
    /// Adresse physique non alignée sur 4 Kio ou au-delà de 52 bits.
    Frame,
    NotPresent,
    /// Entrée du chemin utilisateur sans le bit U/S.
    NotUser,
    NotWritable,
    /// Entrée du mappage noyau accessible au ring 3.
    KernelUser,
    NotUserAddress,
    /// RSP utilisateur non aligné sur 16 octets.
    StackAlignment,
}

impl Ring3Error {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "page frame is misaligned or beyond 52 bits",
            Self::NotPresent => "page table entry is not present",
            Self::NotUser => "user path page table entry lacks the user bit",
            Self::NotWritable => "user page table entry is not writable",
            Self::KernelUser => "kernel mapping is reachable from ring 3",
            Self::NotUserAddress => "address is outside the user region",
            Self::StackAlignment => "user stack is not 16-byte aligned",
        }
    }
}

/// Vrai si le CS sauvegardé par une entrée d'interruption désigne le ring 3.
pub const fn from_user(saved_cs: u64) -> bool {
    saved_cs & 3 == 3
}

/// Indices PML4, PDPT, PD et PT d'une adresse virtuelle.
pub const fn indices(va: u64) -> [usize; 4] {
    [
        ((va >> 39) & 0x1FF) as usize,
        ((va >> 30) & 0x1FF) as usize,
        ((va >> 21) & 0x1FF) as usize,
        ((va >> 12) & 0x1FF) as usize,
    ]
}

pub const fn is_user_address(va: u64) -> bool {
    va >= USER_BASE && va < USER_LIMIT
}

/// Entrée de table de pages vers la page ou la table `frame`.
pub fn table_entry(frame: u64, user: bool, writable: bool) -> Result<u64, Ring3Error> {
    if frame & !PTE_FRAME != 0 {
        return Err(Ring3Error::Frame);
    }
    let mut entry = frame | PTE_PRESENT;
    if writable {
        entry |= PTE_WRITABLE;
    }
    if user {
        entry |= PTE_USER;
    }
    Ok(entry)
}

/// Entrée d'un niveau du chemin vers une page utilisateur.
pub fn check_user_entry(entry: u64, writable: bool) -> Result<(), Ring3Error> {
    if entry & PTE_PRESENT == 0 {
        return Err(Ring3Error::NotPresent);
    }
    if entry & PTE_USER == 0 {
        return Err(Ring3Error::NotUser);
    }
    if writable && entry & PTE_WRITABLE == 0 {
        return Err(Ring3Error::NotWritable);
    }
    Ok(())
}

/// Entrée du mappage identité du noyau : présente et jamais accessible au ring 3.
pub fn check_kernel_entry(entry: u64) -> Result<(), Ring3Error> {
    if entry & PTE_PRESENT == 0 {
        return Err(Ring3Error::NotPresent);
    }
    if entry & PTE_USER != 0 {
        return Err(Ring3Error::KernelUser);
    }
    Ok(())
}

/// Cadre empilé avant `iretq` vers le ring 3, dans l'ordre où `iretq` le dépile.
/// Disposition C : le stub `elyon_enter_user` lit ses champs aux offsets 0, 8,
/// 16, 24 et 32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct IretFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub fn iret_frame(rip: u64, rsp: u64) -> Result<IretFrame, Ring3Error> {
    // RSP peut valoir la fin exacte de la pile : il désigne alors son sommet.
    if !is_user_address(rip) || !is_user_address(rsp.wrapping_sub(1)) {
        return Err(Ring3Error::NotUserAddress);
    }
    if rsp & 0xF != 0 {
        return Err(Ring3Error::StackAlignment);
    }
    Ok(IretFrame {
        rip,
        cs: u64::from(segments::user_selector(LAYOUT.user_code)),
        rflags: USER_RFLAGS,
        rsp,
        ss: u64::from(segments::user_selector(LAYOUT.user_data)),
    })
}
