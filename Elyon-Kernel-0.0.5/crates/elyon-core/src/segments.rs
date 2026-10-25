//! Segments et pile noyau d'entrée (K005-01), sans état global.
//!
//! `syscall` prend CS et SS noyau à partir de `STAR[47:32]` ; `sysret` en mode
//! 64 bits prend SS = `STAR[63:48]` + 8 et CS = `STAR[63:48]` + 16, RPL 3
//! forcé. La GDT doit donc enchaîner code noyau, données noyau, puis données
//! utilisateur et code utilisateur. Ce module fixe cette disposition, calcule
//! `STAR`, décode les descripteurs relus dans la GDT chargée et calcule la pile
//! noyau `RSP0` qu'une entrée depuis le ring 3 prendra pour une tâche.

/// Sélecteurs de la GDT de chaque CPU (index × 8, RPL 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub kernel_code: u16,
    pub kernel_data: u16,
    pub user_data: u16,
    pub user_code: u16,
    /// Descripteur système de 16 octets : deux entrées.
    pub tss: u16,
}

pub const LAYOUT: Layout = Layout {
    kernel_code: 0x08,
    kernel_data: 0x10,
    user_data: 0x18,
    user_code: 0x20,
    tss: 0x28,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentError {
    /// La disposition ne permet pas `syscall`/`sysret`.
    StarLayout,
    TooShort,
    Null,
    KernelCode,
    KernelData,
    UserData,
    UserCode,
    Tss,
    Rsp0,
    /// Le TSS donnerait au ring 3 un bitmap de permissions d'E/S.
    IoBitmap,
    /// Le RSP0 relu n'est pas le sommet de la pile où la tâche s'exécute.
    RspOutside,
    /// Le RSP0 relu tombe dans une pile réservée (IST, boot).
    Rsp0Forbidden,
}

impl SegmentError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StarLayout => "GDT layout is incompatible with syscall/sysret",
            Self::TooShort => "GDT is too short for the kernel layout",
            Self::Null => "first GDT entry is not null",
            Self::KernelCode => {
                "kernel code segment is not a present ring 0 long-mode code segment"
            }
            Self::KernelData => "kernel data segment is not a present ring 0 writable data segment",
            Self::UserData => "user data segment is not a present ring 3 writable data segment",
            Self::UserCode => "user code segment is not a present ring 3 long-mode code segment",
            Self::Tss => "TSS descriptor is not a present ring 0 64-bit TSS",
            Self::Rsp0 => "task stack gives no valid RSP0",
            Self::IoBitmap => "TSS gives ring 3 an I/O permission bitmap",
            Self::RspOutside => "RSP0 is not the top of the stack the task runs on",
            Self::Rsp0Forbidden => "RSP0 lies inside a reserved stack",
        }
    }
}

/// Sélecteur avec RPL 3, tel que le ring 3 le charge.
pub const fn user_selector(selector: u16) -> u16 {
    selector | 3
}

/// Valeur de `STAR` pour la disposition : bits 47:32 pour `syscall`, 63:48
/// pour `sysret` (RPL 3), bits 31:0 inutilisés en mode 64 bits.
pub fn star(layout: &Layout) -> Result<u64, SegmentError> {
    let kernel_ok = layout.kernel_data == layout.kernel_code + 8;
    let user_ok = layout.user_code == layout.user_data + 8 && layout.user_data >= 8;
    if !kernel_ok || !user_ok || layout.kernel_code & 7 != 0 || layout.user_data & 7 != 0 {
        return Err(SegmentError::StarLayout);
    }
    let sysret_base = u64::from(user_selector(layout.user_data - 8));
    Ok((sysret_base << 48) | (u64::from(layout.kernel_code) << 32))
}

/// Champs utiles d'un descripteur de segment de 8 octets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorInfo {
    pub present: bool,
    pub dpl: u8,
    /// Descripteur système (S = 0) : TSS, LDT, portes.
    pub system: bool,
    /// Type sur 4 bits (bits 43:40).
    pub kind: u8,
    pub executable: bool,
    /// Données : écriture permise. Code : lecture permise.
    pub writable: bool,
    /// Code 64 bits (L).
    pub long: bool,
    /// D/B : doit valoir 0 pour un code 64 bits.
    pub default_big: bool,
}

pub const fn decode(raw: u64) -> DescriptorInfo {
    let kind = ((raw >> 40) & 0xF) as u8;
    DescriptorInfo {
        present: raw & (1 << 47) != 0,
        dpl: ((raw >> 45) & 3) as u8,
        system: raw & (1 << 44) == 0,
        kind,
        executable: kind & 0b1000 != 0,
        writable: kind & 0b0010 != 0,
        long: raw & (1 << 53) != 0,
        default_big: raw & (1 << 54) != 0,
    }
}

/// Bit 2 du type : « conforming » pour un code, « expand-down » pour des
/// données. Aucun des deux n'a sa place dans la GDT du noyau.
const KIND_CONFORMING_OR_EXPAND_DOWN: u8 = 0b0100;

/// Limite d'un TSS 64 bits sans bitmap de permissions d'E/S : 104 octets - 1.
pub const TSS_LIMIT: u32 = 0x67;

fn is_code(raw: u64, dpl: u8) -> bool {
    let d = decode(raw);
    d.present
        && !d.system
        && d.executable
        && d.kind & KIND_CONFORMING_OR_EXPAND_DOWN == 0
        && d.long
        && !d.default_big
        && d.dpl == dpl
}

fn is_data(raw: u64, dpl: u8) -> bool {
    let d = decode(raw);
    d.present
        && !d.system
        && !d.executable
        && d.writable
        && d.kind & KIND_CONFORMING_OR_EXPAND_DOWN == 0
        && d.dpl == dpl
}

/// Limite d'un descripteur : bits 15:0 et 51:48.
const fn limit_of(raw: u64) -> u32 {
    ((raw & 0xFFFF) | ((raw >> 32) & 0xF_0000)) as u32
}

/// TSS 64 bits disponible (0x9) ou occupé (0xB, après `ltr`), ring 0, sans
/// place pour un bitmap d'E/S.
fn is_tss(raw: u64) -> bool {
    let d = decode(raw);
    d.present
        && d.system
        && (d.kind == 0x9 || d.kind == 0xB)
        && d.dpl == 0
        && limit_of(raw) == TSS_LIMIT
}

/// Un `iomap_base` au-delà de la limite du TSS signifie « pas de bitmap » :
/// le ring 3 n'a accès à aucun port (IOPL 0).
pub fn check_iomap(iomap_base: u16, limit: u32) -> Result<(), SegmentError> {
    if u32::from(iomap_base) <= limit {
        return Err(SegmentError::IoBitmap);
    }
    Ok(())
}

/// Vérifie les entrées relues dans la GDT chargée contre la disposition.
pub fn check_gdt(entries: &[u64], layout: &Layout) -> Result<(), SegmentError> {
    let at = |selector: u16| entries.get(usize::from(selector >> 3)).copied();
    // Le TSS occupe deux entrées.
    if at(layout.tss + 8).is_none() {
        return Err(SegmentError::TooShort);
    }
    let entry = |selector: u16| at(selector).unwrap_or(0);
    if entries.first() != Some(&0) {
        return Err(SegmentError::Null);
    }
    if !is_code(entry(layout.kernel_code), 0) {
        return Err(SegmentError::KernelCode);
    }
    if !is_data(entry(layout.kernel_data), 0) {
        return Err(SegmentError::KernelData);
    }
    if !is_data(entry(layout.user_data), 3) {
        return Err(SegmentError::UserData);
    }
    if !is_code(entry(layout.user_code), 3) {
        return Err(SegmentError::UserCode);
    }
    if !is_tss(entry(layout.tss)) {
        return Err(SegmentError::Tss);
    }
    Ok(())
}

/// `RSP0` d'une tâche dont la pile noyau est `[start, end)` : son sommet,
/// aligné sur 16 octets comme l'exige l'entrée d'interruption.
pub fn rsp0_for(start: u64, end: u64) -> Result<u64, SegmentError> {
    let top = end & !0xF;
    if start == 0 || top <= start || !is_canonical(top) {
        return Err(SegmentError::Rsp0);
    }
    Ok(top)
}

/// Adresse canonique sur 48 bits : bits 63:47 tous égaux.
pub const fn is_canonical(address: u64) -> bool {
    let upper = address >> 47;
    upper == 0 || upper == 0x1_FFFF
}

/// Contrôle indépendant du RSP0 relu par une tâche dans le TSS de son CPU,
/// contre son RSP réel `rsp` : la tâche s'exécute sur la pile dont RSP0 est
/// le sommet, donc `rsp` est juste en dessous, à moins de `stack_size`.
/// RSP0 ne doit tomber dans aucune pile réservée `[start, end)`.
pub fn check_observed_rsp0(
    rsp0: u64,
    rsp: u64,
    stack_size: u64,
    forbidden: &[(u64, u64)],
) -> Result<(), SegmentError> {
    if rsp >= rsp0 || rsp0 - rsp > stack_size {
        return Err(SegmentError::RspOutside);
    }
    if forbidden
        .iter()
        .any(|&(start, end)| rsp0 > start && rsp0 <= end)
    {
        return Err(SegmentError::Rsp0Forbidden);
    }
    Ok(())
}
