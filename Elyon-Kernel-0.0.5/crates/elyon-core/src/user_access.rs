//! SMEP, SMAP et copies entre espace utilisateur et noyau (K005-04), sans
//! état global.
//!
//! SMEP interdit au noyau d'exécuter une page utilisateur ; SMAP lui interdit
//! d'y lire ou d'y écrire, sauf entre `stac` et `clac` (RFLAGS.AC). Le noyau
//! n'ouvre cette fenêtre que dans sa routine de copie, sur une plage validée
//! ici d'abord : entièrement dans la région utilisateur, sans débordement,
//! bornée. Une #PF sur l'instruction de copie, venue du noyau, sur une adresse
//! utilisateur et avec AC levé, reprend au point de reprise de la copie, qui
//! rend une erreur ; toute autre faute reste fatale.

use crate::ring3::{self, USER_BASE};

pub const CR4_UMIP: u64 = 1 << 11;
pub const CR4_SMEP: u64 = 1 << 20;
pub const CR4_SMAP: u64 = 1 << 21;
/// CPUID.(EAX=07H,ECX=0):EBX.SMEP[7], EBX.SMAP[20] et ECX.UMIP[2].
pub const CPUID7_EBX_SMEP: u32 = 1 << 7;
pub const CPUID7_EBX_SMAP: u32 = 1 << 20;
pub const CPUID7_ECX_UMIP: u32 = 1 << 2;
const CPUID_LEAF_7: u32 = 7;
pub const RFLAGS_AC: u64 = 1 << 18;

pub const VECTOR_PAGE_FAULT: u64 = 14;
/// Bits du code d'erreur d'une #PF.
pub const PF_PRESENT: u64 = 1 << 0;
pub const PF_WRITE: u64 = 1 << 1;
pub const PF_USER: u64 = 1 << 2;
pub const PF_INSTRUCTION: u64 = 1 << 4;

/// Plus grande copie acceptée en une fois.
pub const MAX_COPY_LEN: u64 = 1 << 20;

/// Pages de la qualification, dans la table de pages de la sonde K005-02 :
/// une page de code en lecture seule, une page de données, une page jamais
/// mappée.
pub const PROBE_CODE_VA: u64 = USER_BASE + 4 * ring3::PAGE_SIZE;
pub const PROBE_DATA_VA: u64 = USER_BASE + 5 * ring3::PAGE_SIZE;
pub const PROBE_UNMAPPED_VA: u64 = USER_BASE + 6 * ring3::PAGE_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserAccessError {
    SmepUnsupported,
    SmapUnsupported,
    SmepDisabled,
    SmapDisabled,
    /// Le CPU gère UMIP mais CR4.UMIP est nul.
    UmipDisabled,
    Empty,
    TooLong,
    /// La fin de la plage dépasse 2^64.
    Overflow,
    /// La plage sort de la région utilisateur.
    NotUser,
    /// La copie a pris une #PF : page absente ou accès refusé.
    Fault,
    /// La faute relevée n'a pas la signature d'une violation SMAP.
    SmapFault,
    /// La faute relevée n'a pas la signature d'une violation SMEP.
    SmepFault,
}

impl UserAccessError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SmepUnsupported => "CPU does not support SMEP",
            Self::SmapUnsupported => "CPU does not support SMAP",
            Self::SmepDisabled => "CR4.SMEP is clear",
            Self::SmapDisabled => "CR4.SMAP is clear",
            Self::UmipDisabled => "CPU supports UMIP but CR4.UMIP is clear",
            Self::Empty => "user range is empty",
            Self::TooLong => "user range exceeds the copy limit",
            Self::Overflow => "user range wraps around",
            Self::NotUser => "range is outside the user region",
            Self::Fault => "user copy faulted",
            Self::SmapFault => "fault is not a SMAP violation",
            Self::SmepFault => "fault is not a SMEP violation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub smep: bool,
    pub smap: bool,
    pub umip: bool,
}

/// Protections annoncées par la feuille CPUID 7, ignorée si `max_leaf` ne
/// l'atteint pas.
pub const fn features(max_leaf: u32, leaf7_ebx: u32, leaf7_ecx: u32) -> Features {
    if max_leaf < CPUID_LEAF_7 {
        return Features {
            smep: false,
            smap: false,
            umip: false,
        };
    }
    Features {
        smep: leaf7_ebx & CPUID7_EBX_SMEP != 0,
        smap: leaf7_ebx & CPUID7_EBX_SMAP != 0,
        umip: leaf7_ecx & CPUID7_ECX_UMIP != 0,
    }
}

/// SMEP et SMAP sont exigés : les stubs d'entrée exécutent `clac`, qui lève
/// #UD sur un CPU sans SMAP.
pub const fn check_support(features: Features) -> Result<(), UserAccessError> {
    if !features.smep {
        return Err(UserAccessError::SmepUnsupported);
    }
    if !features.smap {
        return Err(UserAccessError::SmapUnsupported);
    }
    Ok(())
}

/// Bits de CR4 à activer.
pub const fn cr4_bits(features: Features) -> u64 {
    let mut bits = 0;
    if features.smep {
        bits |= CR4_SMEP;
    }
    if features.smap {
        bits |= CR4_SMAP;
    }
    if features.umip {
        bits |= CR4_UMIP;
    }
    bits
}

pub const fn check_cr4(cr4: u64, features: Features) -> Result<(), UserAccessError> {
    if features.smep && cr4 & CR4_SMEP == 0 {
        return Err(UserAccessError::SmepDisabled);
    }
    if features.smap && cr4 & CR4_SMAP == 0 {
        return Err(UserAccessError::SmapDisabled);
    }
    if features.umip && cr4 & CR4_UMIP == 0 {
        return Err(UserAccessError::UmipDisabled);
    }
    Ok(())
}

/// Plage utilisateur validée : `[start, start + len)` dans la région
/// utilisateur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserRange {
    pub start: u64,
    pub len: u64,
}

impl UserRange {
    pub const fn end(self) -> u64 {
        self.start + self.len
    }
}

pub const fn user_range(start: u64, len: u64) -> Result<UserRange, UserAccessError> {
    if len == 0 {
        return Err(UserAccessError::Empty);
    }
    if len > MAX_COPY_LEN {
        return Err(UserAccessError::TooLong);
    }
    let Some(end) = start.checked_add(len) else {
        return Err(UserAccessError::Overflow);
    };
    if !ring3::is_user_address(start) || !ring3::is_user_address(end - 1) {
        return Err(UserAccessError::NotUser);
    }
    Ok(UserRange { start, len })
}

/// Exception relevée par le dispatcher : vecteur, cadre matériel, CR2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultSite {
    pub vector: u64,
    pub cs: u64,
    pub rip: u64,
    pub error_code: u64,
    pub cr2: u64,
    pub rflags: u64,
}

const fn kernel_page_fault(site: &FaultSite) -> bool {
    site.vector == VECTOR_PAGE_FAULT && !ring3::from_user(site.cs) && site.error_code & PF_USER == 0
}

/// RIP de reprise d'une faute de copie, ou `None` si la faute est fatale.
/// Seule l'instruction de copie (`access`) est reprise, pour une #PF du
/// noyau sur une adresse utilisateur, en lecture ou en écriture de données,
/// pendant la fenêtre `stac`/`clac`.
pub const fn copy_resume(site: &FaultSite, access: u64, fixup: u64) -> Option<u64> {
    if !kernel_page_fault(site)
        || site.rip != access
        || site.error_code & PF_INSTRUCTION != 0
        || site.rflags & RFLAGS_AC == 0
        || !ring3::is_user_address(site.cr2)
    {
        return None;
    }
    Some(fixup)
}

/// Lecture de données du noyau sur une page utilisateur présente, hors de la
/// fenêtre `stac`/`clac`.
pub const fn check_smap_fault(site: &FaultSite, address: u64) -> Result<(), UserAccessError> {
    if !kernel_page_fault(site)
        || site.error_code & (PF_PRESENT | PF_WRITE | PF_INSTRUCTION) != PF_PRESENT
        || site.cr2 != address
        || site.rflags & RFLAGS_AC != 0
    {
        return Err(UserAccessError::SmapFault);
    }
    Ok(())
}

/// Exécution par le noyau d'une page utilisateur présente, en `target`.
pub const fn check_smep_fault(site: &FaultSite, target: u64) -> Result<(), UserAccessError> {
    if !kernel_page_fault(site)
        || site.error_code & (PF_PRESENT | PF_INSTRUCTION) != PF_PRESENT | PF_INSTRUCTION
        || site.rip != target
        || site.cr2 != target
    {
        return Err(UserAccessError::SmepFault);
    }
    Ok(())
}
