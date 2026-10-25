//! Contexte CPU préservé par le scheduler (K004-05), sans accès matériel.
//!
//! Décrit quels registres chaque chemin de switch sauvegarde, fournit des motifs
//! uniques par tâche et par registre, et compare un état observé à l'attendu.

/// Registres généraux hors RSP, dans l'ordre du buffer écrit par la boucle
/// d'intégrité du noyau.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpr {
    Rax,
    Rbx,
    Rcx,
    Rdx,
    Rsi,
    Rdi,
    Rbp,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

pub const GPR_COUNT: usize = 15;

/// Bit DF de RFLAGS : la tâche le positionne, le stub d'IRQ l'efface (`cld`).
pub const RFLAGS_DF: u64 = 1 << 10;

impl Gpr {
    pub const ALL: [Gpr; GPR_COUNT] = [
        Gpr::Rax,
        Gpr::Rbx,
        Gpr::Rcx,
        Gpr::Rdx,
        Gpr::Rsi,
        Gpr::Rdi,
        Gpr::Rbp,
        Gpr::R8,
        Gpr::R9,
        Gpr::R10,
        Gpr::R11,
        Gpr::R12,
        Gpr::R13,
        Gpr::R14,
        Gpr::R15,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Gpr::Rax => "rax",
            Gpr::Rbx => "rbx",
            Gpr::Rcx => "rcx",
            Gpr::Rdx => "rdx",
            Gpr::Rsi => "rsi",
            Gpr::Rdi => "rdi",
            Gpr::Rbp => "rbp",
            Gpr::R8 => "r8",
            Gpr::R9 => "r9",
            Gpr::R10 => "r10",
            Gpr::R11 => "r11",
            Gpr::R12 => "r12",
            Gpr::R13 => "r13",
            Gpr::R14 => "r14",
            Gpr::R15 => "r15",
        }
    }
}

/// Chemin par lequel une tâche a perdu le CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPath {
    /// IRQ timer : le stub sauvegarde les quinze registres, le CPU empile
    /// RIP/CS/RFLAGS/RSP/SS ; tout l'état visible de la tâche est restauré.
    Preemption,
    /// `yield_now()`/`exit_now()` : un appel de fonction ; seuls les registres
    /// callee-saved de l'ABI SysV doivent survivre.
    Cooperative,
}

pub const fn preserved(path: SwitchPath, gpr: Gpr) -> bool {
    match path {
        SwitchPath::Preemption => true,
        SwitchPath::Cooperative => matches!(
            gpr,
            Gpr::Rbx | Gpr::Rbp | Gpr::R12 | Gpr::R13 | Gpr::R14 | Gpr::R15
        ),
    }
}

/// Motif 64 bits propre à `(task, gpr)` : tâche et registre occupent des champs
/// de bits distincts, les bits hauts sont toujours positionnés.
pub const fn pattern(task: u8, gpr: Gpr) -> u64 {
    0xE105_0000_0000_0000
        | ((task as u64) << 40)
        | ((gpr.index() as u64 + 1) << 32)
        | 0x5A5A_0000
        | ((task as u64) << 8)
        | gpr.index() as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterFile {
    pub gprs: [u64; GPR_COUNT],
    pub rflags: u64,
}

/// État que la boucle d'intégrité de la tâche `task` doit maintenir.
pub const fn expected(task: u8) -> RegisterFile {
    let mut gprs = [0u64; GPR_COUNT];
    let mut index = 0;
    while index < GPR_COUNT {
        gprs[index] = pattern(task, Gpr::ALL[index]);
        index += 1;
    }
    RegisterFile {
        gprs,
        rflags: RFLAGS_DF,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorruptionReport {
    /// Bit `i` positionné si le registre `Gpr::ALL[i]` diffère de l'attendu.
    pub corrupted_mask: u16,
    pub df_lost: bool,
}

impl CorruptionReport {
    pub const fn is_clean(&self) -> bool {
        self.corrupted_mask == 0 && !self.df_lost
    }

    pub fn first_corrupted(&self) -> Option<Gpr> {
        Gpr::ALL
            .iter()
            .copied()
            .find(|gpr| self.corrupted_mask & (1 << gpr.index()) != 0)
    }
}

/// Compare les registres généraux et le seul bit DF : les autres bits de RFLAGS
/// (CF, ZF, IF…) évoluent légitimement entre deux observations.
pub fn compare(expected: &RegisterFile, observed: &RegisterFile) -> CorruptionReport {
    let mut corrupted_mask = 0u16;
    for gpr in Gpr::ALL {
        if expected.gprs[gpr.index()] != observed.gprs[gpr.index()] {
            corrupted_mask |= 1 << gpr.index();
        }
    }
    CorruptionReport {
        corrupted_mask,
        df_lost: expected.rflags & RFLAGS_DF != 0 && observed.rflags & RFLAGS_DF == 0,
    }
}
