//! Entrée « paranoïaque » NMI, #DF et #MC (K005-03), sans état global.
//!
//! Ces trois vecteurs peuvent arriver n'importe où, y compris entre le
//! `swapgs` de sortie et l'`iretq` vers le ring 3 : le CS sauvegardé est alors
//! celui du noyau alors que GS est déjà le GS utilisateur. Le stub ne s'y fie
//! donc pas. Chacun tourne sur sa propre pile IST, au sommet de laquelle le
//! noyau a rangé le GS_BASE de ce CPU ; le stub relit le MSR GS_BASE, le
//! remplace par ce mot s'ils diffèrent, et remet la valeur d'entrée avant
//! `iretq`. KERNEL_GS_BASE n'est jamais touché.

use crate::segments::is_canonical;

pub const VECTOR_NMI: u8 = 2;
pub const VECTOR_DOUBLE_FAULT: u8 = 8;
pub const VECTOR_MACHINE_CHECK: u8 = 18;

/// Numéros d'IST tels qu'écrits dans le champ IST de la porte (1 à 7).
pub const IST_DOUBLE_FAULT: u16 = 1;
pub const IST_NMI: u16 = 2;
pub const IST_MACHINE_CHECK: u16 = 3;

/// Octets réservés au sommet de chaque pile IST : le GS_BASE noyau du CPU,
/// puis un mot nul qui garde le pointeur IST aligné sur 16.
pub const IST_SLOT_BYTES: u64 = 16;
/// Taille minimale d'une pile IST paranoïaque.
pub const MIN_IST_SIZE: u64 = 16 * 1024;
/// Position du mot GS noyau depuis RSP une fois le stub entré : GS d'entrée,
/// 15 registres généraux, vecteur et code d'erreur, cadre matériel (RIP, CS,
/// RFLAGS, RSP, SS) poussé juste sous le pointeur IST.
pub const SLOT_OFFSET: u64 = (1 + 15 + 2 + 5) * 8;

/// Type d'une porte d'interruption 64 bits (IF effacé à l'entrée).
const GATE_KIND_INTERRUPT: u8 = 0xE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParanoidError {
    /// Pile IST nulle, trop petite ou non canonique.
    IstSpan,
    /// Deux piles IST se chevauchent.
    IstOverlap,
    /// Porte absente, de trappe, ou atteignable depuis le ring 3.
    Gate,
    /// Porte sans sa pile IST dédiée.
    GateIst,
    /// Porte vers un autre gestionnaire ou un autre sélecteur.
    GateTarget,
    /// L'entrée n'a pas vu le GS attendu : la condition testée n'a pas eu lieu.
    EntryGs,
    /// Le gestionnaire a tourné avec un autre GS que celui du noyau.
    DispatchGs,
    /// L'index lu via GS n'est pas celui du CPU.
    Index,
    /// Le gestionnaire ne tournait pas sur sa pile IST.
    Stack,
    /// Le CPU annonce MCE mais CR4.MCE est nul : un #MC arrêterait le CPU.
    MceDisabled,
}

impl ParanoidError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IstSpan => "IST stack is null, too small or non-canonical",
            Self::IstOverlap => "two IST stacks overlap",
            Self::Gate => "gate is not a present ring 0 interrupt gate",
            Self::GateIst => "gate does not use its dedicated IST",
            Self::GateTarget => "gate targets another handler or selector",
            Self::EntryGs => "entry did not see the expected GS_BASE",
            Self::DispatchGs => "handler ran without the kernel GS_BASE",
            Self::Index => "handler read another CPU index through GS",
            Self::Stack => "handler did not run on its IST stack",
            Self::MceDisabled => "CPU supports MCE but CR4.MCE is clear",
        }
    }
}

/// Pile IST dédiée d'un vecteur paranoïaque.
pub const fn ist_for_vector(vector: u8) -> Option<u16> {
    match vector {
        VECTOR_NMI => Some(IST_NMI),
        VECTOR_DOUBLE_FAULT => Some(IST_DOUBLE_FAULT),
        VECTOR_MACHINE_CHECK => Some(IST_MACHINE_CHECK),
        _ => None,
    }
}

fn check_span(start: u64, end: u64) -> Result<(), ParanoidError> {
    if start == 0
        || end <= start
        || end - start < MIN_IST_SIZE
        || !is_canonical(start)
        || !is_canonical(end - 1)
    {
        return Err(ParanoidError::IstSpan);
    }
    Ok(())
}

/// Valeur du pointeur IST du TSS pour la pile `[start, end)` : sommet aligné
/// sur 16, moins le mot GS noyau. Le processeur pousse son cadre juste dessous.
pub fn ist_pointer(start: u64, end: u64) -> Result<u64, ParanoidError> {
    check_span(start, end)?;
    Ok((end & !0xF) - IST_SLOT_BYTES)
}

/// Toutes les piles valides et deux à deux disjointes.
pub fn check_spans(spans: &[(u64, u64)]) -> Result<(), ParanoidError> {
    for &(start, end) in spans {
        check_span(start, end)?;
    }
    for (index, &(start, end)) in spans.iter().enumerate() {
        if spans[index + 1..]
            .iter()
            .any(|&(other_start, other_end)| start < other_end && other_start < end)
        {
            return Err(ParanoidError::IstOverlap);
        }
    }
    Ok(())
}

/// Champs utiles d'une porte de l'IDT 64 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub present: bool,
    pub dpl: u8,
    pub kind: u8,
    pub ist: u16,
    pub selector: u16,
    pub handler: u64,
}

pub const fn decode_gate(low: u64, high: u64) -> Gate {
    Gate {
        present: low & (1 << 47) != 0,
        dpl: ((low >> 45) & 3) as u8,
        kind: ((low >> 40) & 0xF) as u8,
        ist: ((low >> 32) & 7) as u16,
        selector: (low >> 16) as u16,
        handler: (low & 0xFFFF) | (((low >> 48) & 0xFFFF) << 16) | ((high & 0xFFFF_FFFF) << 32),
    }
}

/// Porte relue dans l'IDT chargée contre ce qu'elle doit être.
pub fn check_gate(
    low: u64,
    high: u64,
    selector: u16,
    handler: u64,
    ist: u16,
) -> Result<(), ParanoidError> {
    let gate = decode_gate(low, high);
    if !gate.present || gate.dpl != 0 || gate.kind != GATE_KIND_INTERRUPT {
        return Err(ParanoidError::Gate);
    }
    if gate.ist != ist {
        return Err(ParanoidError::GateIst);
    }
    if gate.selector != selector || gate.handler != handler {
        return Err(ParanoidError::GateTarget);
    }
    Ok(())
}

/// Ce que le gestionnaire a vu, relevé sans passer par GS sauf `dispatch_index`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// GS_BASE à l'entrée du stub, avant toute correction.
    pub entry_gs: u64,
    /// GS_BASE relu par le gestionnaire.
    pub dispatch_gs: u64,
    /// Index lu via `gs:[0]` par le gestionnaire.
    pub dispatch_index: u64,
    /// RSP du gestionnaire.
    pub rsp: u64,
}

/// Ce que la qualification attend pour une entrée sur un CPU donné.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    /// Zone per-CPU de ce CPU.
    pub kernel_gs: u64,
    /// GS_BASE au moment de l'événement : celui du noyau, ou le GS
    /// utilisateur dans la fenêtre `swapgs`/`iretq`.
    pub entry_gs: u64,
    pub index: u64,
    /// Pile IST du vecteur sur ce CPU.
    pub ist: (u64, u64),
}

pub fn check_observation(
    observation: &Observation,
    expected: &Expected,
) -> Result<(), ParanoidError> {
    if observation.entry_gs != expected.entry_gs {
        return Err(ParanoidError::EntryGs);
    }
    if observation.dispatch_gs != expected.kernel_gs {
        return Err(ParanoidError::DispatchGs);
    }
    if observation.dispatch_index != expected.index {
        return Err(ParanoidError::Index);
    }
    let (start, end) = expected.ist;
    if observation.rsp <= start || observation.rsp >= end {
        return Err(ParanoidError::Stack);
    }
    Ok(())
}

/// Vrai si le stub a dû remplacer GS_BASE : l'entrée tombait hors du GS noyau.
pub const fn replaced(observation: &Observation, kernel_gs: u64) -> bool {
    observation.entry_gs != kernel_gs
}

/// CPUID.01H:EDX.MCE (bit 7) et CR4.MCE (bit 6).
const CPUID_EDX_MCE: u32 = 1 << 7;
const CR4_MCE: u64 = 1 << 6;

/// Sans CR4.MCE, un #MC met le CPU en arrêt (shutdown) au lieu d'entrer dans
/// le gestionnaire : il doit être actif dès que le CPU le gère.
pub const fn check_mce(cr4: u64, cpuid_1_edx: u32) -> Result<(), ParanoidError> {
    if cpuid_1_edx & CPUID_EDX_MCE != 0 && cr4 & CR4_MCE == 0 {
        return Err(ParanoidError::MceDisabled);
    }
    Ok(())
}
