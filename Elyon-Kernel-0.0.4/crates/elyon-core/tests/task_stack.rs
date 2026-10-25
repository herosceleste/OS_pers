//! Tests hôte des piles de tâches noyau (K004-02) : validation des régions,
//! chevauchements, frame initial et canaris.

use elyon_core::task_stack::{
    self, CANARY, CANARY_QWORDS, FrameSpec, INITIAL_FRAME_QWORDS, StackError, StackRegion,
};

const KIB: u64 = 1024;
const IDENTITY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const MIN_SIZE: u64 = 64 * KIB;

fn region(start: u64, size: u64) -> StackRegion {
    StackRegion {
        start,
        end: start + size,
    }
}

// --- Validation d'une région ------------------------------------------------------

#[test]
fn page_aligned_region_inside_identity_map_is_valid() {
    let r = region(0x0040_0000, MIN_SIZE);
    assert_eq!(
        task_stack::validate_region(r, MIN_SIZE, IDENTITY_LIMIT),
        Ok(())
    );
}

#[test]
fn empty_or_inverted_region_is_rejected() {
    let empty = StackRegion {
        start: 0x0040_0000,
        end: 0x0040_0000,
    };
    let inverted = StackRegion {
        start: 0x0050_0000,
        end: 0x0040_0000,
    };
    assert_eq!(
        task_stack::validate_region(empty, MIN_SIZE, IDENTITY_LIMIT),
        Err(StackError::Empty)
    );
    assert_eq!(
        task_stack::validate_region(inverted, MIN_SIZE, IDENTITY_LIMIT),
        Err(StackError::Empty)
    );
}

#[test]
fn region_not_page_aligned_is_rejected() {
    let r = region(0x0040_0010, MIN_SIZE);
    assert_eq!(
        task_stack::validate_region(r, MIN_SIZE, IDENTITY_LIMIT),
        Err(StackError::Misaligned)
    );
}

#[test]
fn region_smaller_than_minimum_is_rejected() {
    let r = region(0x0040_0000, MIN_SIZE - 4 * KIB);
    assert_eq!(
        task_stack::validate_region(r, MIN_SIZE, IDENTITY_LIMIT),
        Err(StackError::TooSmall)
    );
}

#[test]
fn region_crossing_identity_limit_is_rejected() {
    let r = region(IDENTITY_LIMIT - 32 * KIB, MIN_SIZE);
    assert_eq!(
        task_stack::validate_region(r, MIN_SIZE, IDENTITY_LIMIT),
        Err(StackError::OutsideIdentityMap)
    );
}

#[test]
fn region_ending_exactly_at_identity_limit_is_valid() {
    let r = region(IDENTITY_LIMIT - MIN_SIZE, MIN_SIZE);
    assert_eq!(
        task_stack::validate_region(r, MIN_SIZE, IDENTITY_LIMIT),
        Ok(())
    );
}

// --- Chevauchements ---------------------------------------------------------------

#[test]
fn adjacent_regions_do_not_overlap() {
    let a = region(0x0040_0000, MIN_SIZE);
    let b = region(0x0040_0000 + MIN_SIZE, MIN_SIZE);
    assert!(!task_stack::overlaps(a, b));
    assert_eq!(task_stack::find_overlap(&[a, b]), None);
}

#[test]
fn partially_overlapping_regions_are_detected_both_ways() {
    let a = region(0x0040_0000, MIN_SIZE);
    let b = region(0x0040_0000 + MIN_SIZE / 2, MIN_SIZE);
    assert!(task_stack::overlaps(a, b));
    assert!(task_stack::overlaps(b, a));
}

#[test]
fn find_overlap_reports_the_first_conflicting_pair() {
    let a = region(0x0040_0000, MIN_SIZE);
    let b = region(0x0060_0000, MIN_SIZE);
    let c = region(0x0040_0000 + 4 * KIB, MIN_SIZE);
    assert_eq!(task_stack::find_overlap(&[a, b, c]), Some((0, 2)));
}

#[test]
fn identical_regions_overlap() {
    let a = region(0x0040_0000, MIN_SIZE);
    assert_eq!(task_stack::find_overlap(&[a, a]), Some((0, 1)));
}

// --- Position du RSP --------------------------------------------------------------

#[test]
fn rsp_is_inside_only_within_start_inclusive_end_exclusive() {
    let r = region(0x0040_0000, MIN_SIZE);
    assert!(task_stack::rsp_inside(r, r.start));
    assert!(task_stack::rsp_inside(r, r.end - 8));
    assert!(!task_stack::rsp_inside(r, r.end));
    assert!(!task_stack::rsp_inside(r, r.start - 8));
}

#[test]
fn rsp_inside_the_canary_zone_counts_as_overflow() {
    let r = region(0x0040_0000, MIN_SIZE);
    let canary_top = r.start + (CANARY_QWORDS as u64) * 8;
    assert!(task_stack::rsp_overflowed(r, canary_top - 8));
    assert!(!task_stack::rsp_overflowed(r, canary_top));
}

// --- Frame initial ----------------------------------------------------------------

const ENTRY: u64 = 0xFFFF_8000_0010_0000;
const ARG: u64 = 0x2A;
const TRAMPOLINE: u64 = 0xFFFF_8000_0020_0000;
const RFLAGS: u64 = 0x2;

fn spec(stack: StackRegion) -> FrameSpec {
    FrameSpec {
        stack,
        entry: ENTRY,
        arg: ARG,
        trampoline: TRAMPOLINE,
        rflags: RFLAGS,
    }
}

#[test]
fn initial_frame_sits_at_the_top_of_the_stack() {
    let r = region(0x0040_0000, MIN_SIZE);
    let (rsp, _) = task_stack::initial_frame(spec(r)).unwrap();
    assert_eq!(rsp, r.end - (INITIAL_FRAME_QWORDS as u64) * 8);
}

#[test]
fn initial_frame_matches_context_switch_pop_order() {
    // elyon_context_switch dépile r15, r14, r13, r12, rbp, rbx, rflags puis `ret`.
    // Le trampoline appelle r12 (entrée) avec rbx (argument) dans rdi.
    let r = region(0x0040_0000, MIN_SIZE);
    let (_, frame) = task_stack::initial_frame(spec(r)).unwrap();
    assert_eq!(frame, [0, 0, 0, ENTRY, 0, ARG, RFLAGS, TRAMPOLINE]);
}

#[test]
fn stack_is_16_byte_aligned_after_ret_into_the_trampoline() {
    // Après les 7 pops et le `ret`, RSP vaut la fin de pile : le `call` du
    // trampoline laisse alors RSP ≡ 8 (mod 16) à l'entrée, comme l'exige SysV.
    let r = region(0x0040_0000, MIN_SIZE);
    let (rsp, _) = task_stack::initial_frame(spec(r)).unwrap();
    let after_ret = rsp + (INITIAL_FRAME_QWORDS as u64) * 8;
    assert_eq!(after_ret % 16, 0);
    assert_eq!(rsp % 16, 0);
}

#[test]
fn initial_frame_rejects_misaligned_stack_top() {
    let r = StackRegion {
        start: 0x0040_0000,
        end: 0x0040_0000 + MIN_SIZE - 8,
    };
    assert_eq!(
        task_stack::initial_frame(spec(r)),
        Err(StackError::Misaligned)
    );
}

#[test]
fn initial_frame_rejects_null_entry_or_trampoline() {
    let r = region(0x0040_0000, MIN_SIZE);
    let mut no_entry = spec(r);
    no_entry.entry = 0;
    let mut no_trampoline = spec(r);
    no_trampoline.trampoline = 0;
    assert_eq!(
        task_stack::initial_frame(no_entry),
        Err(StackError::InvalidEntry)
    );
    assert_eq!(
        task_stack::initial_frame(no_trampoline),
        Err(StackError::InvalidEntry)
    );
}

#[test]
fn initial_frame_rejects_stack_too_small_for_frame_and_canary() {
    let r = region(0x0040_0000, 64);
    assert_eq!(
        task_stack::initial_frame(spec(r)),
        Err(StackError::TooSmall)
    );
}

// --- Canaris ----------------------------------------------------------------------

#[test]
fn canary_pattern_is_recognised() {
    let words = [CANARY; CANARY_QWORDS];
    assert!(task_stack::canary_intact(&words));
}

#[test]
fn any_overwritten_canary_word_is_detected() {
    for index in 0..CANARY_QWORDS {
        let mut words = [CANARY; CANARY_QWORDS];
        words[index] = 0;
        assert!(!task_stack::canary_intact(&words), "word {index}");
    }
}

#[test]
fn canary_check_requires_the_full_canary_length() {
    let words = [CANARY; CANARY_QWORDS - 1];
    assert!(!task_stack::canary_intact(&words));
}

#[test]
fn canary_is_not_a_trivial_value() {
    assert_ne!(CANARY, 0);
    assert_ne!(CANARY, u64::MAX);
}
