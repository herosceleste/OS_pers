//! Tests hôte de SMEP, SMAP et des copies entre espace utilisateur et noyau
//! (K005-04) : prise en charge CPUID, bits de CR4, plages utilisateur,
//! reprise d'une faute de copie et signatures des fautes SMAP et SMEP.

use elyon_core::ring3::{USER_BASE, USER_LIMIT};
use elyon_core::user_access::{self, FaultSite, UserAccessError, UserRange};

const CPUID7_EBX_BOTH: u32 = user_access::CPUID7_EBX_SMEP | user_access::CPUID7_EBX_SMAP;
const KERNEL_CS: u64 = 0x08;
const USER_CS: u64 = 0x23;
const ACCESS: u64 = 0x0020_1000;
const FIXUP: u64 = 0x0020_1010;

// --- Prise en charge et CR4 ----------------------------------------------------

#[test]
fn features_are_read_from_cpuid_leaf_7() {
    let features = user_access::features(7, CPUID7_EBX_BOTH, user_access::CPUID7_ECX_UMIP);
    assert!(features.smep && features.smap && features.umip);
    let none = user_access::features(7, 0, 0);
    assert!(!none.smep && !none.smap && !none.umip);
}

#[test]
fn leaf_7_is_ignored_when_the_cpu_does_not_report_it() {
    // Au-delà de la feuille maximale, CPUID rend des valeurs sans rapport.
    let features = user_access::features(6, u32::MAX, u32::MAX);
    assert!(!features.smep && !features.smap && !features.umip);
}

#[test]
fn smep_and_smap_are_required() {
    let both = user_access::features(7, CPUID7_EBX_BOTH, 0);
    assert_eq!(user_access::check_support(both), Ok(()));
    let no_smap = user_access::features(7, user_access::CPUID7_EBX_SMEP, 0);
    assert_eq!(
        user_access::check_support(no_smap),
        Err(UserAccessError::SmapUnsupported)
    );
    let no_smep = user_access::features(7, user_access::CPUID7_EBX_SMAP, 0);
    assert_eq!(
        user_access::check_support(no_smep),
        Err(UserAccessError::SmepUnsupported)
    );
}

#[test]
fn cr4_bits_follow_the_features() {
    let all = user_access::features(7, CPUID7_EBX_BOTH, user_access::CPUID7_ECX_UMIP);
    assert_eq!(
        user_access::cr4_bits(all),
        user_access::CR4_SMEP | user_access::CR4_SMAP | user_access::CR4_UMIP
    );
    // UMIP n'est activé que si le CPU le gère.
    let no_umip = user_access::features(7, CPUID7_EBX_BOTH, 0);
    assert_eq!(
        user_access::cr4_bits(no_umip),
        user_access::CR4_SMEP | user_access::CR4_SMAP
    );
}

#[test]
fn cr4_must_carry_every_supported_protection() {
    let all = user_access::features(7, CPUID7_EBX_BOTH, user_access::CPUID7_ECX_UMIP);
    let cr4 = 0x0040_0668 | user_access::cr4_bits(all);
    assert_eq!(user_access::check_cr4(cr4, all), Ok(()));
    assert_eq!(
        user_access::check_cr4(cr4 & !user_access::CR4_SMEP, all),
        Err(UserAccessError::SmepDisabled)
    );
    assert_eq!(
        user_access::check_cr4(cr4 & !user_access::CR4_SMAP, all),
        Err(UserAccessError::SmapDisabled)
    );
    assert_eq!(
        user_access::check_cr4(cr4 & !user_access::CR4_UMIP, all),
        Err(UserAccessError::UmipDisabled)
    );
    let no_umip = user_access::features(7, CPUID7_EBX_BOTH, 0);
    assert_eq!(
        user_access::check_cr4(cr4 & !user_access::CR4_UMIP, no_umip),
        Ok(())
    );
}

// --- Plages utilisateur ----------------------------------------------------------

#[test]
fn a_range_inside_the_user_region_is_accepted() {
    let range = user_access::user_range(USER_BASE + 0x5000, 64).unwrap();
    assert_eq!(
        range,
        UserRange {
            start: USER_BASE + 0x5000,
            len: 64
        }
    );
    assert_eq!(range.end(), USER_BASE + 0x5040);
    // Le dernier octet de la région utilisateur est atteignable.
    assert!(user_access::user_range(USER_LIMIT - 16, 16).is_ok());
    assert!(user_access::user_range(USER_BASE, user_access::MAX_COPY_LEN).is_ok());
}

#[test]
fn an_empty_or_oversized_range_is_refused() {
    assert_eq!(
        user_access::user_range(USER_BASE, 0),
        Err(UserAccessError::Empty)
    );
    assert_eq!(
        user_access::user_range(USER_BASE, user_access::MAX_COPY_LEN + 1),
        Err(UserAccessError::TooLong)
    );
}

#[test]
fn a_kernel_address_is_refused() {
    // Mappage identité du noyau, sous la région utilisateur.
    assert_eq!(
        user_access::user_range(0x0020_0000, 64),
        Err(UserAccessError::NotUser)
    );
    // Une plage qui commence juste sous USER_BASE.
    assert_eq!(
        user_access::user_range(USER_BASE - 8, 16),
        Err(UserAccessError::NotUser)
    );
    // Moitié haute canonique.
    assert_eq!(
        user_access::user_range(0xFFFF_8000_0000_0000, 64),
        Err(UserAccessError::NotUser)
    );
}

#[test]
fn a_range_crossing_the_user_limit_is_refused() {
    assert_eq!(
        user_access::user_range(USER_LIMIT - 8, 16),
        Err(UserAccessError::NotUser)
    );
}

#[test]
fn a_wrapping_range_is_refused() {
    assert_eq!(
        user_access::user_range(u64::MAX - 4, 16),
        Err(UserAccessError::Overflow)
    );
}

// --- Reprise d'une faute de copie ------------------------------------------------

fn copy_fault() -> FaultSite {
    FaultSite {
        vector: user_access::VECTOR_PAGE_FAULT,
        cs: KERNEL_CS,
        rip: ACCESS,
        error_code: 0,
        cr2: USER_BASE + 0x6000,
        rflags: 0x46 | user_access::RFLAGS_AC,
    }
}

#[test]
fn a_page_fault_on_the_copy_instruction_resumes_at_the_fixup() {
    assert_eq!(
        user_access::copy_resume(&copy_fault(), ACCESS, FIXUP),
        Some(FIXUP)
    );
    // Écriture refusée sur une page utilisateur en lecture seule.
    let mut site = copy_fault();
    site.error_code = user_access::PF_PRESENT | user_access::PF_WRITE;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), Some(FIXUP));
}

#[test]
fn a_fault_elsewhere_in_the_kernel_is_not_resumed() {
    let mut site = copy_fault();
    site.rip = ACCESS + 2;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

#[test]
fn a_fault_from_ring_3_is_not_resumed() {
    let mut site = copy_fault();
    site.cs = USER_CS;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

#[test]
fn only_a_page_fault_is_resumed() {
    // #GP : adresse non canonique, jamais produite par une plage validée.
    let mut site = copy_fault();
    site.vector = 13;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

#[test]
fn a_fault_on_a_kernel_address_during_a_copy_is_not_resumed() {
    // Le tampon noyau de la copie est en faute : bogue du noyau, fatal.
    let mut site = copy_fault();
    site.cr2 = 0x0040_0000;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

#[test]
fn a_copy_fault_without_ac_is_a_smap_violation_and_is_not_resumed() {
    // La copie a touché une page utilisateur sans `stac` : bogue du noyau.
    let mut site = copy_fault();
    site.rflags &= !user_access::RFLAGS_AC;
    site.error_code = user_access::PF_PRESENT;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

#[test]
fn an_instruction_fetch_fault_is_not_resumed() {
    let mut site = copy_fault();
    site.error_code = user_access::PF_PRESENT | user_access::PF_INSTRUCTION;
    assert_eq!(user_access::copy_resume(&site, ACCESS, FIXUP), None);
}

// --- Signatures des fautes SMAP et SMEP -------------------------------------------

const DATA: u64 = USER_BASE + 0x5000;
const CODE: u64 = USER_BASE + 0x4000;

fn smap_fault() -> FaultSite {
    FaultSite {
        vector: user_access::VECTOR_PAGE_FAULT,
        cs: KERNEL_CS,
        rip: ACCESS,
        error_code: user_access::PF_PRESENT,
        cr2: DATA,
        rflags: 0x46,
    }
}

fn smep_fault() -> FaultSite {
    FaultSite {
        vector: user_access::VECTOR_PAGE_FAULT,
        cs: KERNEL_CS,
        rip: CODE,
        error_code: user_access::PF_PRESENT | user_access::PF_INSTRUCTION,
        cr2: CODE,
        rflags: 0x46,
    }
}

#[test]
fn a_supervisor_read_of_a_present_user_page_is_a_smap_fault() {
    assert_eq!(user_access::check_smap_fault(&smap_fault(), DATA), Ok(()));
}

#[test]
fn a_smap_fault_must_be_a_present_supervisor_data_read() {
    for error_code in [
        0,
        user_access::PF_PRESENT | user_access::PF_USER,
        user_access::PF_PRESENT | user_access::PF_WRITE,
        user_access::PF_PRESENT | user_access::PF_INSTRUCTION,
    ] {
        let mut site = smap_fault();
        site.error_code = error_code;
        assert_eq!(
            user_access::check_smap_fault(&site, DATA),
            Err(UserAccessError::SmapFault)
        );
    }
}

#[test]
fn a_smap_fault_on_another_address_or_with_ac_is_refused() {
    let mut site = smap_fault();
    site.cr2 = DATA + 8;
    assert_eq!(
        user_access::check_smap_fault(&site, DATA),
        Err(UserAccessError::SmapFault)
    );
    // Avec AC, SMAP autorise l'accès : la faute aurait une autre cause.
    let mut site = smap_fault();
    site.rflags |= user_access::RFLAGS_AC;
    assert_eq!(
        user_access::check_smap_fault(&site, DATA),
        Err(UserAccessError::SmapFault)
    );
    let mut site = smap_fault();
    site.cs = USER_CS;
    assert_eq!(
        user_access::check_smap_fault(&site, DATA),
        Err(UserAccessError::SmapFault)
    );
}

#[test]
fn a_supervisor_fetch_from_a_user_page_is_a_smep_fault() {
    assert_eq!(user_access::check_smep_fault(&smep_fault(), CODE), Ok(()));
}

#[test]
fn a_smep_fault_must_be_a_present_supervisor_fetch_at_the_target() {
    let mut site = smep_fault();
    site.error_code = user_access::PF_PRESENT;
    assert_eq!(
        user_access::check_smep_fault(&site, CODE),
        Err(UserAccessError::SmepFault)
    );
    let mut site = smep_fault();
    site.error_code = user_access::PF_INSTRUCTION;
    assert_eq!(
        user_access::check_smep_fault(&site, CODE),
        Err(UserAccessError::SmepFault)
    );
    let mut site = smep_fault();
    site.rip = CODE + 1;
    assert_eq!(
        user_access::check_smep_fault(&site, CODE),
        Err(UserAccessError::SmepFault)
    );
    let mut site = smep_fault();
    site.cr2 = CODE + 0x1000;
    assert_eq!(
        user_access::check_smep_fault(&site, CODE),
        Err(UserAccessError::SmepFault)
    );
    let mut site = smep_fault();
    site.cs = USER_CS;
    assert_eq!(
        user_access::check_smep_fault(&site, CODE),
        Err(UserAccessError::SmepFault)
    );
}

#[test]
fn the_probe_pages_live_in_the_first_user_page_table() {
    // Même table de pages que la sonde K005-02, hors des GS utilisateur des
    // sondes (USER_BASE + 0x10_0000 et au-delà), jamais mappés.
    for va in [
        user_access::PROBE_CODE_VA,
        user_access::PROBE_DATA_VA,
        user_access::PROBE_UNMAPPED_VA,
    ] {
        assert!((USER_BASE + 2 * 0x1000..USER_BASE + 0x10_0000).contains(&va));
        assert_eq!(va & 0xFFF, 0);
    }
    assert_ne!(user_access::PROBE_CODE_VA, user_access::PROBE_DATA_VA);
    assert_ne!(user_access::PROBE_DATA_VA, user_access::PROBE_UNMAPPED_VA);
}

#[test]
fn every_error_has_a_message() {
    for error in [
        UserAccessError::SmepUnsupported,
        UserAccessError::SmapUnsupported,
        UserAccessError::SmepDisabled,
        UserAccessError::SmapDisabled,
        UserAccessError::UmipDisabled,
        UserAccessError::Empty,
        UserAccessError::TooLong,
        UserAccessError::Overflow,
        UserAccessError::NotUser,
        UserAccessError::Fault,
        UserAccessError::SmapFault,
        UserAccessError::SmepFault,
    ] {
        assert!(!error.as_str().is_empty());
    }
}
