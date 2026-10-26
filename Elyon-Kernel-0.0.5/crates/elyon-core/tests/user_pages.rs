//! Tests hôte des pages utilisateur (K005-05) : entrées exactes des pages de
//! code et de données, NX, découpage d'une grande page du mappage identité
//! pour en retirer une frame, et protocole de TLB shootdown.

use elyon_core::user_pages::{self, PageKind, UserPageError};

const FRAME: u64 = 0x0012_3000;
const GIB: u64 = 1 << 30;
const IDENTITY_LIMIT: u64 = 4 * GIB;

// --- Entrées des pages utilisateur ------------------------------------------------

#[test]
fn a_code_page_is_user_present_read_only_and_executable() {
    let entry = user_pages::user_leaf(FRAME, PageKind::Code).unwrap();
    assert_eq!(
        entry,
        FRAME | user_pages::PTE_PRESENT | user_pages::PTE_USER
    );
}

#[test]
fn a_data_page_is_user_writable_and_never_executable() {
    let entry = user_pages::user_leaf(FRAME, PageKind::Data).unwrap();
    assert_eq!(
        entry,
        FRAME
            | user_pages::PTE_PRESENT
            | user_pages::PTE_USER
            | user_pages::PTE_WRITABLE
            | user_pages::PTE_NX
    );
}

#[test]
fn a_misaligned_or_oversized_frame_is_refused() {
    assert_eq!(
        user_pages::user_leaf(FRAME + 8, PageKind::Data),
        Err(UserPageError::Frame)
    );
    assert_eq!(
        user_pages::user_leaf(1 << 52, PageKind::Data),
        Err(UserPageError::Frame)
    );
}

#[test]
fn a_user_leaf_is_checked_exactly() {
    for kind in [PageKind::Code, PageKind::Data] {
        let entry = user_pages::user_leaf(FRAME, kind).unwrap();
        assert_eq!(user_pages::check_user_leaf(entry, FRAME, kind), Ok(()));
        // Bits Accessed et Dirty posés par le processeur : tolérés.
        assert_eq!(
            user_pages::check_user_leaf(entry | 0x60, FRAME, kind),
            Ok(())
        );
    }
}

#[test]
fn a_user_leaf_with_other_rights_or_frame_is_refused() {
    let code = user_pages::user_leaf(FRAME, PageKind::Code).unwrap();
    let data = user_pages::user_leaf(FRAME, PageKind::Data).unwrap();
    for (entry, kind) in [
        // Code inscriptible.
        (code | user_pages::PTE_WRITABLE, PageKind::Code),
        // Données exécutables.
        (data & !user_pages::PTE_NX, PageKind::Data),
        // Sans le bit U/S.
        (data & !user_pages::PTE_USER, PageKind::Data),
        // Page globale.
        (data | user_pages::PTE_GLOBAL, PageKind::Data),
        // Absente.
        (code & !user_pages::PTE_PRESENT, PageKind::Code),
        // Autre frame.
        (data + 0x1000, PageKind::Data),
    ] {
        assert_eq!(
            user_pages::check_user_leaf(entry, FRAME, kind),
            Err(UserPageError::Leaf)
        );
    }
}

// --- NX -------------------------------------------------------------------------

#[test]
fn nx_is_read_from_the_extended_cpuid_leaf() {
    assert!(user_pages::nx_supported(
        0x8000_0008,
        user_pages::CPUID_EXT1_EDX_NX
    ));
    assert!(!user_pages::nx_supported(0x8000_0008, 0));
    // Feuille étendue 1 absente.
    assert!(!user_pages::nx_supported(0x8000_0000, u32::MAX));
}

#[test]
fn efer_must_enable_nx() {
    assert_eq!(
        user_pages::check_efer(0x0D01 | user_pages::EFER_NXE),
        Ok(())
    );
    assert_eq!(
        user_pages::check_efer(0x0D01 & !user_pages::EFER_NXE),
        Err(UserPageError::NxDisabled)
    );
}

// --- Mappage identité -------------------------------------------------------------

#[test]
fn a_frame_is_located_in_the_identity_map() {
    assert_eq!(user_pages::identity_slot(0, IDENTITY_LIMIT), Ok((0, 0, 0)));
    // 1 Gio + 2 Mio + 3 pages.
    let phys = GIB + 2 * 1024 * 1024 + 3 * 0x1000;
    assert_eq!(
        user_pages::identity_slot(phys, IDENTITY_LIMIT),
        Ok((1, 1, 3))
    );
    assert_eq!(
        user_pages::identity_slot(IDENTITY_LIMIT - 0x1000, IDENTITY_LIMIT),
        Ok((3, 511, 511))
    );
}

#[test]
fn a_frame_outside_the_identity_map_or_misaligned_is_refused() {
    assert_eq!(
        user_pages::identity_slot(IDENTITY_LIMIT, IDENTITY_LIMIT),
        Err(UserPageError::OutsideIdentity)
    );
    assert_eq!(
        user_pages::identity_slot(FRAME + 1, IDENTITY_LIMIT),
        Err(UserPageError::Frame)
    );
}

const HUGE: u64 = 1 << 7;
const PAT_2M: u64 = 1 << 12;
const PAT_4K: u64 = 1 << 7;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const BASE_2M: u64 = 0x4060_0000;

#[test]
fn a_huge_page_splits_into_the_same_small_pages() {
    let pde = BASE_2M | user_pages::PTE_PRESENT | user_pages::PTE_WRITABLE | HUGE;
    assert_eq!(
        user_pages::small_entry(pde, 0),
        Ok(BASE_2M | user_pages::PTE_PRESENT | user_pages::PTE_WRITABLE)
    );
    assert_eq!(
        user_pages::small_entry(pde, 511),
        Ok((BASE_2M + 511 * 0x1000) | user_pages::PTE_PRESENT | user_pages::PTE_WRITABLE)
    );
}

#[test]
fn a_split_keeps_the_cache_attributes_and_moves_pat() {
    // MMIO non cacheable (PWT|PCD) et PAT : bit 12 d'une PDE, bit 7 d'une PTE.
    let pde =
        BASE_2M | user_pages::PTE_PRESENT | user_pages::PTE_WRITABLE | HUGE | PWT | PCD | PAT_2M;
    assert_eq!(
        user_pages::small_entry(pde, 2),
        Ok((BASE_2M + 0x2000)
            | user_pages::PTE_PRESENT
            | user_pages::PTE_WRITABLE
            | PWT
            | PCD
            | PAT_4K)
    );
}

#[test]
fn only_a_present_supervisor_huge_page_is_split() {
    let pde = BASE_2M | user_pages::PTE_PRESENT | user_pages::PTE_WRITABLE | HUGE;
    assert_eq!(
        user_pages::small_entry(pde & !HUGE, 0),
        Err(UserPageError::NotHuge)
    );
    assert_eq!(
        user_pages::small_entry(pde & !user_pages::PTE_PRESENT, 0),
        Err(UserPageError::NotPresent)
    );
    // Le mappage identité ne porte jamais U/S : une telle PDE est refusée.
    assert_eq!(
        user_pages::small_entry(pde | user_pages::PTE_USER, 0),
        Err(UserPageError::Leaf)
    );
    assert_eq!(user_pages::small_entry(pde, 512), Err(UserPageError::Frame));
}

// --- TLB shootdown ------------------------------------------------------------------

#[test]
fn a_shootdown_targets_every_other_online_cpu() {
    assert_eq!(user_pages::shootdown_targets(4, 0), 0b1110);
    assert_eq!(user_pages::shootdown_targets(4, 2), 0b1011);
    assert_eq!(user_pages::shootdown_targets(1, 0), 0);
}

#[test]
fn a_shootdown_completes_once_every_target_acknowledged_its_generation() {
    let targets = 0b1110;
    let mut acks = [0u64; 4];
    assert!(!user_pages::all_acked(&acks, targets, 5));
    acks[1] = 5;
    acks[2] = 5;
    assert!(!user_pages::all_acked(&acks, targets, 5));
    // Un acquittement d'une génération antérieure ne compte pas.
    acks[3] = 4;
    assert!(!user_pages::all_acked(&acks, targets, 5));
    acks[3] = 5;
    assert!(user_pages::all_acked(&acks, targets, 5));
    // Le CPU émetteur (0) n'est pas attendu.
    assert_eq!(acks[0], 0);
}

#[test]
fn the_frame_mask_keeps_bits_51_to_12() {
    assert_eq!(user_pages::PTE_FRAME, 0x000F_FFFF_FFFF_F000);
    let leaf = user_pages::user_leaf(FRAME, PageKind::Data).unwrap();
    assert_eq!(leaf & user_pages::PTE_FRAME, FRAME);
}

#[test]
fn every_error_has_a_message() {
    for error in [
        UserPageError::Frame,
        UserPageError::Leaf,
        UserPageError::NotPresent,
        UserPageError::NotHuge,
        UserPageError::OutsideIdentity,
        UserPageError::AliasPresent,
        UserPageError::NxUnsupported,
        UserPageError::NxDisabled,
        UserPageError::OutOfMemory,
    ] {
        assert!(!error.as_str().is_empty());
    }
}
