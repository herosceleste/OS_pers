//! Tests hôte des segments (K005-01) : disposition de la GDT compatible
//! `syscall`/`sysret`, valeur de `STAR`, décodage et contrôle des descripteurs
//! chargés, pile noyau `RSP0` d'une tâche.

use elyon_core::segments::{self, Layout, SegmentError};

// Descripteurs tels que les écrit le noyau (crate x86_64) puis `ltr`.
const NULL: u64 = 0;
const KERNEL_CODE: u64 = 0x00af_9b00_0000_ffff;
const KERNEL_DATA: u64 = 0x00cf_9300_0000_ffff;
const USER_DATA: u64 = 0x00cf_f300_0000_ffff;
const USER_CODE: u64 = 0x00af_fb00_0000_ffff;
const TSS_BUSY_LOW: u64 = 0x0000_8b00_0000_0067;
const TSS_HIGH: u64 = 0x0000_0000_0000_0000;

fn gdt() -> [u64; 7] {
    [
        NULL,
        KERNEL_CODE,
        KERNEL_DATA,
        USER_DATA,
        USER_CODE,
        TSS_BUSY_LOW,
        TSS_HIGH,
    ]
}

// --- Disposition et STAR -----------------------------------------------------

#[test]
fn layout_places_user_data_just_before_user_code() {
    let layout = segments::LAYOUT;
    assert_eq!(layout.kernel_code, 0x08);
    assert_eq!(layout.kernel_data, 0x10);
    assert_eq!(layout.user_data, 0x18);
    assert_eq!(layout.user_code, 0x20);
    assert_eq!(layout.tss, 0x28);
}

#[test]
fn star_gives_syscall_and_sysret_the_right_segments() {
    let star = segments::star(&segments::LAYOUT).unwrap();
    let syscall_base = (star >> 32) & 0xFFFF;
    let sysret_base = (star >> 48) & 0xFFFF;
    // syscall : CS = base, SS = base + 8.
    assert_eq!(syscall_base, 0x08);
    assert_eq!(syscall_base + 8, 0x10);
    // sysret 64 bits : SS = base + 8, CS = base + 16, RPL 3 forcé.
    assert_eq!((sysret_base & !3) + 8, 0x18);
    assert_eq!((sysret_base & !3) + 16, 0x20);
    assert_eq!(sysret_base & 3, 3);
    assert_eq!(star & 0xFFFF_FFFF, 0);
}

#[test]
fn star_refuses_user_code_before_user_data() {
    let layout = Layout {
        user_data: 0x20,
        user_code: 0x18,
        ..segments::LAYOUT
    };
    assert_eq!(segments::star(&layout), Err(SegmentError::StarLayout));
}

#[test]
fn star_refuses_kernel_data_not_following_kernel_code() {
    let layout = Layout {
        kernel_data: 0x18,
        ..segments::LAYOUT
    };
    assert_eq!(segments::star(&layout), Err(SegmentError::StarLayout));
}

#[test]
fn user_selectors_carry_rpl_3() {
    assert_eq!(segments::user_selector(0x18), 0x1b);
    assert_eq!(segments::user_selector(0x20), 0x23);
}

// --- Décodage des descripteurs -----------------------------------------------

#[test]
fn decode_reads_privilege_and_long_mode() {
    let code = segments::decode(USER_CODE);
    assert!(code.present && !code.system && code.executable && code.long && !code.default_big);
    assert_eq!(code.dpl, 3);
    let data = segments::decode(KERNEL_DATA);
    assert!(data.present && !data.executable && data.writable);
    assert_eq!(data.dpl, 0);
    let tss = segments::decode(TSS_BUSY_LOW);
    assert!(tss.present && tss.system);
    assert_eq!(tss.kind, 0xB);
}

// --- Contrôle de la GDT chargée ----------------------------------------------

#[test]
fn the_kernel_gdt_is_accepted() {
    assert_eq!(segments::check_gdt(&gdt(), &segments::LAYOUT), Ok(()));
}

#[test]
fn an_available_tss_is_accepted_too() {
    let mut entries = gdt();
    entries[5] = 0x0000_8900_0000_0067;
    assert_eq!(segments::check_gdt(&entries, &segments::LAYOUT), Ok(()));
}

#[test]
fn a_short_gdt_is_refused() {
    assert_eq!(
        segments::check_gdt(&gdt()[..6], &segments::LAYOUT),
        Err(SegmentError::TooShort)
    );
}

#[test]
fn a_non_null_first_entry_is_refused() {
    let mut entries = gdt();
    entries[0] = KERNEL_DATA;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::Null)
    );
}

#[test]
fn user_code_with_kernel_privilege_is_refused() {
    let mut entries = gdt();
    entries[4] = KERNEL_CODE;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserCode)
    );
}

#[test]
fn user_code_without_long_mode_is_refused() {
    let mut entries = gdt();
    entries[4] = USER_CODE & !(1 << 53);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserCode)
    );
}

#[test]
fn kernel_code_reachable_from_ring_3_is_refused() {
    let mut entries = gdt();
    entries[1] = USER_CODE;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::KernelCode)
    );
}

#[test]
fn read_only_user_data_is_refused() {
    let mut entries = gdt();
    entries[3] = USER_DATA & !(1 << 41);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserData)
    );
}

#[test]
fn executable_kernel_data_is_refused() {
    let mut entries = gdt();
    entries[2] = KERNEL_CODE;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::KernelData)
    );
}

#[test]
fn a_missing_tss_is_refused() {
    let mut entries = gdt();
    entries[5] = USER_DATA;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::Tss)
    );
}

#[test]
fn a_tss_visible_to_ring_3_is_refused() {
    let mut entries = gdt();
    entries[5] = TSS_BUSY_LOW | (3 << 45);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::Tss)
    );
}

#[test]
fn a_not_present_segment_is_refused() {
    let mut entries = gdt();
    entries[3] = USER_DATA & !(1 << 47);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserData)
    );
}

// --- RSP0 ---------------------------------------------------------------------

#[test]
fn rsp0_is_the_aligned_top_of_the_task_stack() {
    assert_eq!(segments::rsp0_for(0x10_0000, 0x11_0000), Ok(0x11_0000));
    assert_eq!(segments::rsp0_for(0x10_0000, 0x11_0008), Ok(0x11_0000));
}

#[test]
fn rsp0_refuses_an_empty_or_inverted_stack() {
    assert_eq!(
        segments::rsp0_for(0x11_0000, 0x11_0000),
        Err(SegmentError::Rsp0)
    );
    assert_eq!(
        segments::rsp0_for(0x11_0000, 0x10_0000),
        Err(SegmentError::Rsp0)
    );
    assert_eq!(segments::rsp0_for(0, 0), Err(SegmentError::Rsp0));
}

#[test]
fn rsp0_refuses_a_stack_smaller_than_one_alignment() {
    assert_eq!(
        segments::rsp0_for(0x11_0001, 0x11_000f),
        Err(SegmentError::Rsp0)
    );
}

#[test]
fn rsp0_refuses_a_non_canonical_top() {
    assert_eq!(
        segments::rsp0_for(0x0000_7fff_ffff_0000, 0x0000_8000_0000_0000),
        Err(SegmentError::Rsp0)
    );
    assert_eq!(
        segments::rsp0_for(0x0000_7fff_fff0_0000, 0x0000_7fff_ffff_fff0),
        Ok(0x0000_7fff_ffff_fff0)
    );
}

// --- Revue ECC K005-01 : types refusés et bitmap d'E/S ---------------------------

#[test]
fn conforming_user_code_is_refused() {
    let mut entries = gdt();
    entries[4] = USER_CODE | (1 << 42);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserCode)
    );
}

#[test]
fn conforming_kernel_code_is_refused() {
    let mut entries = gdt();
    entries[1] = KERNEL_CODE | (1 << 42);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::KernelCode)
    );
}

#[test]
fn expand_down_user_data_is_refused() {
    let mut entries = gdt();
    entries[3] = USER_DATA | (1 << 42);
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::UserData)
    );
}

#[test]
fn a_tss_long_enough_for_an_io_bitmap_is_refused() {
    let mut entries = gdt();
    entries[5] = (TSS_BUSY_LOW & !0xFFFF) | 0x2067;
    assert_eq!(
        segments::check_gdt(&entries, &segments::LAYOUT),
        Err(SegmentError::Tss)
    );
}

#[test]
fn an_io_bitmap_base_past_the_tss_limit_means_no_port_access() {
    assert_eq!(segments::check_iomap(0x68, 0x67), Ok(()));
    assert_eq!(
        segments::check_iomap(0x60, 0x67),
        Err(SegmentError::IoBitmap)
    );
}

// --- RSP0 observé, contrôlé contre le RSP réel de la tâche --------------------

#[test]
fn an_observed_rsp0_just_above_the_live_rsp_is_accepted() {
    let forbidden = [(0x20_0000, 0x20_4000)];
    assert_eq!(
        segments::check_observed_rsp0(0x11_0000, 0x10_ff00, 0x1_0000, &forbidden),
        Ok(())
    );
}

#[test]
fn an_observed_rsp0_below_the_live_rsp_is_refused() {
    assert_eq!(
        segments::check_observed_rsp0(0x10_ff00, 0x11_0000, 0x1_0000, &[]),
        Err(SegmentError::RspOutside)
    );
}

#[test]
fn an_observed_rsp0_of_another_stack_is_refused() {
    // RSP0 d'une autre tâche, une pile plus haut : trop loin du RSP réel.
    assert_eq!(
        segments::check_observed_rsp0(0x13_0000, 0x10_ff00, 0x1_0000, &[]),
        Err(SegmentError::RspOutside)
    );
}

#[test]
fn an_observed_rsp0_inside_a_forbidden_stack_is_refused() {
    let forbidden = [(0x10_8000, 0x11_0000)];
    assert_eq!(
        segments::check_observed_rsp0(0x11_0000, 0x10_ff00, 0x1_0000, &forbidden),
        Err(SegmentError::Rsp0Forbidden)
    );
}

#[test]
fn every_error_has_a_message() {
    for error in [
        SegmentError::IoBitmap,
        SegmentError::RspOutside,
        SegmentError::Rsp0Forbidden,
        SegmentError::StarLayout,
        SegmentError::TooShort,
        SegmentError::Null,
        SegmentError::KernelCode,
        SegmentError::KernelData,
        SegmentError::UserData,
        SegmentError::UserCode,
        SegmentError::Tss,
        SegmentError::Rsp0,
    ] {
        assert!(!error.as_str().is_empty());
    }
}
