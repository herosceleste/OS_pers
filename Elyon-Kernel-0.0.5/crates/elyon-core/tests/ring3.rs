//! Tests hôte de la frontière ring 3 minimale (K005-02) : origine d'une entrée,
//! région virtuelle utilisateur, entrées de table de pages et cadre `iretq`.

use elyon_core::ring3::{self, IretFrame, Ring3Error};
use elyon_core::segments;

// --- Origine d'une entrée -------------------------------------------------------

#[test]
fn an_entry_from_ring_3_is_recognised_by_the_saved_cs_rpl() {
    assert!(ring3::from_user(0x23));
    assert!(!ring3::from_user(0x08));
    assert!(!ring3::from_user(0x10));
    // Seuls les deux bits bas comptent : le CS sauvegardé est étendu sur 64 bits.
    assert!(ring3::from_user(0xFFFF_0000_0000_0023));
}

// --- Région utilisateur -------------------------------------------------------

#[test]
fn the_user_region_lives_in_its_own_pml4_entry() {
    let [pml4, pdpt, pd, pt] = ring3::indices(ring3::USER_BASE);
    assert_eq!(pml4, 1);
    assert_eq!((pdpt, pd, pt), (0, 0, 0));
    // Le mappage identité du noyau occupe PML4[0] : aucune page utilisateur n'y tombe.
    assert_ne!(pml4, 0);
}

#[test]
fn the_probe_code_and_stack_pages_are_consecutive_user_pages() {
    assert_eq!(ring3::USER_CODE_VA, ring3::USER_BASE);
    assert_eq!(ring3::USER_STACK_VA, ring3::USER_BASE + 0x1000);
    assert_eq!(ring3::USER_STACK_TOP, ring3::USER_BASE + 0x2000);
    assert_eq!(ring3::indices(ring3::USER_STACK_VA)[3], 1);
    assert!(ring3::is_user_address(ring3::USER_STACK_TOP - 8));
    assert!(!ring3::is_user_address(0x0010_0000));
    assert!(!ring3::is_user_address(0xFFFF_8000_0000_0000));
}

// --- Entrées de table de pages -------------------------------------------------

#[test]
fn a_user_entry_carries_present_and_user_bits() {
    let entry = ring3::table_entry(0x0020_0000, true, true).unwrap();
    assert_eq!(
        entry & 0xFFF,
        ring3::PTE_PRESENT | ring3::PTE_WRITABLE | ring3::PTE_USER
    );
    assert_eq!(entry & !0xFFF, 0x0020_0000);
    assert_eq!(ring3::check_user_entry(entry, true), Ok(()));
}

#[test]
fn a_read_only_user_entry_is_accepted_when_read_only_is_expected() {
    let entry = ring3::table_entry(0x0020_0000, true, false).unwrap();
    assert_eq!(ring3::check_user_entry(entry, false), Ok(()));
    assert_eq!(
        ring3::check_user_entry(entry, true),
        Err(Ring3Error::NotWritable)
    );
}

#[test]
fn a_misaligned_or_too_large_frame_is_refused() {
    assert_eq!(
        ring3::table_entry(0x0020_0800, true, true),
        Err(Ring3Error::Frame)
    );
    assert_eq!(
        ring3::table_entry(1 << 52, true, true),
        Err(Ring3Error::Frame)
    );
}

#[test]
fn a_supervisor_entry_on_the_user_path_is_refused() {
    let entry = ring3::table_entry(0x0020_0000, false, true).unwrap();
    assert_eq!(
        ring3::check_user_entry(entry, true),
        Err(Ring3Error::NotUser)
    );
}

#[test]
fn a_missing_entry_is_refused() {
    assert_eq!(
        ring3::check_user_entry(0, true),
        Err(Ring3Error::NotPresent)
    );
    assert_eq!(ring3::check_kernel_entry(0), Err(Ring3Error::NotPresent));
}

#[test]
fn the_kernel_identity_map_must_stay_supervisor_only() {
    let kernel = 0x0010_0000 | ring3::PTE_PRESENT | ring3::PTE_WRITABLE;
    assert_eq!(ring3::check_kernel_entry(kernel), Ok(()));
    assert_eq!(
        ring3::check_kernel_entry(kernel | ring3::PTE_USER),
        Err(Ring3Error::KernelUser)
    );
}

// --- Cadre iretq vers le ring 3 --------------------------------------------------

#[test]
fn the_iret_frame_enters_ring_3_with_interrupts_enabled() {
    let frame = ring3::iret_frame(ring3::USER_CODE_VA, ring3::USER_STACK_TOP).unwrap();
    assert_eq!(
        frame,
        IretFrame {
            rip: ring3::USER_CODE_VA,
            cs: u64::from(segments::user_selector(segments::LAYOUT.user_code)),
            rflags: 0x202,
            rsp: ring3::USER_STACK_TOP,
            ss: u64::from(segments::user_selector(segments::LAYOUT.user_data)),
        }
    );
    // IOPL 0 : aucun accès aux ports depuis le ring 3.
    assert_eq!(frame.rflags & (3 << 12), 0);
}

#[test]
fn the_iret_frame_layout_matches_the_enter_user_stub() {
    assert_eq!(core::mem::offset_of!(IretFrame, rip), 0);
    assert_eq!(core::mem::offset_of!(IretFrame, cs), 8);
    assert_eq!(core::mem::offset_of!(IretFrame, rflags), 16);
    assert_eq!(core::mem::offset_of!(IretFrame, rsp), 24);
    assert_eq!(core::mem::offset_of!(IretFrame, ss), 32);
    assert_eq!(core::mem::size_of::<IretFrame>(), 40);
}

#[test]
fn the_iret_frame_refuses_a_kernel_target() {
    assert_eq!(
        ring3::iret_frame(0x0010_0000, ring3::USER_STACK_TOP),
        Err(Ring3Error::NotUserAddress)
    );
    assert_eq!(
        ring3::iret_frame(ring3::USER_CODE_VA, 0x0010_0000),
        Err(Ring3Error::NotUserAddress)
    );
    assert_eq!(
        ring3::iret_frame(ring3::USER_CODE_VA, 0),
        Err(Ring3Error::NotUserAddress)
    );
}

#[test]
fn the_iret_frame_refuses_a_misaligned_stack() {
    assert_eq!(
        ring3::iret_frame(ring3::USER_CODE_VA, ring3::USER_STACK_TOP - 8),
        Err(Ring3Error::StackAlignment)
    );
}

#[test]
fn every_error_has_a_message() {
    for error in [
        Ring3Error::Frame,
        Ring3Error::NotPresent,
        Ring3Error::NotUser,
        Ring3Error::NotWritable,
        Ring3Error::KernelUser,
        Ring3Error::NotUserAddress,
        Ring3Error::StackAlignment,
    ] {
        assert!(!error.as_str().is_empty());
    }
}
