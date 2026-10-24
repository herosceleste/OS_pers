//! Piles de tâches noyau (K004-02), sans accès mémoire.
//!
//! Le noyau alloue la pile au PMM, écrit le frame et les canaris ; ce module
//! valide la région, calcule le frame initial attendu par `elyon_context_switch`
//! et juge l'état des canaris.

use elyon_abi::PAGE_SIZE;

/// Nombre de qwords dépilés par `elyon_context_switch` pour une tâche neuve :
/// r15, r14, r13, r12, rbp, rbx, rflags, puis l'adresse de retour.
pub const INITIAL_FRAME_QWORDS: usize = 8;

/// Motif écrit au bas de chaque pile ; une valeur modifiée signale un débordement.
pub const CANARY: u64 = 0xE170_57AC_C0DE_0402;
pub const CANARY_QWORDS: usize = 4;

const QWORD: u64 = 8;
const FRAME_BYTES: u64 = INITIAL_FRAME_QWORDS as u64 * QWORD;
const CANARY_BYTES: u64 = CANARY_QWORDS as u64 * QWORD;
const STACK_ALIGN: u64 = 16;

const SLOT_R12: usize = 3;
const SLOT_RBX: usize = 5;
const SLOT_RFLAGS: usize = 6;
const SLOT_RETURN: usize = 7;

/// Région `[start, end)` d'une pile ; la pile croît vers `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackRegion {
    pub start: u64,
    pub end: u64,
}

impl StackRegion {
    pub const fn size(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    Empty,
    Misaligned,
    TooSmall,
    OutsideIdentityMap,
    InvalidEntry,
}

impl StackError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "task stack region is empty",
            Self::Misaligned => "task stack is not correctly aligned",
            Self::TooSmall => "task stack is smaller than required",
            Self::OutsideIdentityMap => "task stack outside current identity map",
            Self::InvalidEntry => "task entry or trampoline address is null",
        }
    }
}

/// Ce que le noyau veut voir s'exécuter au premier switch vers la tâche.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSpec {
    pub stack: StackRegion,
    /// Fonction d'entrée, transmise au trampoline dans r12.
    pub entry: u64,
    /// Argument de l'entrée, transmis au trampoline dans rbx puis rdi.
    pub arg: u64,
    /// Cible du `ret` final de `elyon_context_switch`.
    pub trampoline: u64,
    pub rflags: u64,
}

/// Vérifie qu'une pile allouée est utilisable : non vide, alignée sur la page,
/// assez grande et entièrement couverte par l'identity map.
pub fn validate_region(
    region: StackRegion,
    min_size: u64,
    identity_limit: u64,
) -> Result<(), StackError> {
    if region.end <= region.start {
        return Err(StackError::Empty);
    }
    if !region.start.is_multiple_of(PAGE_SIZE) || !region.end.is_multiple_of(PAGE_SIZE) {
        return Err(StackError::Misaligned);
    }
    if region.size() < min_size {
        return Err(StackError::TooSmall);
    }
    if region.end > identity_limit {
        return Err(StackError::OutsideIdentityMap);
    }
    Ok(())
}

pub const fn overlaps(a: StackRegion, b: StackRegion) -> bool {
    a.start < b.end && b.start < a.end
}

/// Première paire `(i, j)` avec `i < j` dont les régions se chevauchent.
pub fn find_overlap(regions: &[StackRegion]) -> Option<(usize, usize)> {
    for (i, a) in regions.iter().enumerate() {
        for (offset, b) in regions[i + 1..].iter().enumerate() {
            if overlaps(*a, *b) {
                return Some((i, i + 1 + offset));
            }
        }
    }
    None
}

pub const fn rsp_inside(region: StackRegion, rsp: u64) -> bool {
    rsp >= region.start && rsp < region.end
}

/// Vrai si `rsp` est descendu dans la zone des canaris (ou en dessous).
pub const fn rsp_overflowed(region: StackRegion, rsp: u64) -> bool {
    rsp < region.start + CANARY_BYTES
}

/// Calcule le RSP initial et le contenu du frame, de l'adresse basse vers la
/// haute, tel que `elyon_context_switch` le dépile.
pub fn initial_frame(spec: FrameSpec) -> Result<(u64, [u64; INITIAL_FRAME_QWORDS]), StackError> {
    if spec.entry == 0 || spec.trampoline == 0 {
        return Err(StackError::InvalidEntry);
    }
    let stack = spec.stack;
    if stack.end <= stack.start {
        return Err(StackError::Empty);
    }
    if stack.size() < FRAME_BYTES + CANARY_BYTES {
        return Err(StackError::TooSmall);
    }
    // Le `ret` laisse RSP sur `stack.end` : il doit être aligné sur 16 pour que
    // le `call` du trampoline respecte l'ABI SysV à l'entrée de la tâche.
    if !stack.end.is_multiple_of(STACK_ALIGN) {
        return Err(StackError::Misaligned);
    }

    let mut frame = [0u64; INITIAL_FRAME_QWORDS];
    frame[SLOT_R12] = spec.entry;
    frame[SLOT_RBX] = spec.arg;
    frame[SLOT_RFLAGS] = spec.rflags;
    frame[SLOT_RETURN] = spec.trampoline;

    Ok((stack.end - FRAME_BYTES, frame))
}

/// Vrai si les `CANARY_QWORDS` mots du bas de pile portent encore le motif.
pub fn canary_intact(words: &[u64]) -> bool {
    words.len() == CANARY_QWORDS && words.iter().all(|&word| word == CANARY)
}
