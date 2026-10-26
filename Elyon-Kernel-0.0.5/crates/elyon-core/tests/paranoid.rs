//! Tests hôte de l'entrée « paranoïaque » (K005-03) : piles IST dédiées, mot
//! GS noyau au sommet de chaque pile, portes NMI/#DF/#MC relues dans l'IDT,
//! observations de la qualification et CR4.MCE.

use elyon_core::paranoid::{self, Expected, Observation, ParanoidError};

const KB: u64 = 1024;

/// Porte telle que l'écrit le noyau (`IdtEntry::new`) : 64 bits bas, 64 hauts.
fn gate(handler: u64, selector: u16, options: u16) -> (u64, u64) {
    let low = (handler & 0xFFFF)
        | (u64::from(selector) << 16)
        | (u64::from(options) << 32)
        | (((handler >> 16) & 0xFFFF) << 48);
    (low, handler >> 32)
}

// --- Piles IST ---------------------------------------------------------------

#[test]
fn each_paranoid_vector_has_its_own_ist() {
    assert_eq!(paranoid::IST_DOUBLE_FAULT, 1);
    assert_eq!(paranoid::IST_NMI, 2);
    assert_eq!(paranoid::IST_MACHINE_CHECK, 3);
    assert_eq!(
        paranoid::ist_for_vector(paranoid::VECTOR_NMI),
        Some(paranoid::IST_NMI)
    );
    assert_eq!(
        paranoid::ist_for_vector(paranoid::VECTOR_DOUBLE_FAULT),
        Some(paranoid::IST_DOUBLE_FAULT)
    );
    assert_eq!(
        paranoid::ist_for_vector(paranoid::VECTOR_MACHINE_CHECK),
        Some(paranoid::IST_MACHINE_CHECK)
    );
    assert_eq!(paranoid::ist_for_vector(14), None);
}

#[test]
fn the_ist_pointer_leaves_an_aligned_slot_at_the_top() {
    let start = 0x0040_0000;
    let end = start + 16 * KB;
    let pointer = paranoid::ist_pointer(start, end).unwrap();
    assert_eq!(pointer, end - paranoid::IST_SLOT_BYTES);
    assert_eq!(pointer % 16, 0);
    // Une fin mal alignée est arrondie vers le bas avant de réserver le mot.
    assert_eq!(
        paranoid::ist_pointer(start, end + 8).unwrap(),
        end - paranoid::IST_SLOT_BYTES
    );
}

#[test]
fn an_empty_small_or_non_canonical_ist_is_refused() {
    assert_eq!(
        paranoid::ist_pointer(0, 16 * KB),
        Err(ParanoidError::IstSpan)
    );
    assert_eq!(
        paranoid::ist_pointer(0x0040_0000, 0x0040_0000),
        Err(ParanoidError::IstSpan)
    );
    assert_eq!(
        paranoid::ist_pointer(0x0040_0000, 0x0040_0000 + 4 * KB),
        Err(ParanoidError::IstSpan)
    );
    assert_eq!(
        paranoid::ist_pointer(0x0000_8000_0000_0000, 0x0000_8000_0000_0000 + 16 * KB),
        Err(ParanoidError::IstSpan)
    );
}

#[test]
fn the_slot_offset_matches_the_stub_frame() {
    // GS d'entrée, 15 registres, vecteur, code d'erreur, puis le cadre
    // matériel RIP, CS, RFLAGS, RSP, SS : le mot GS noyau est juste au-dessus.
    assert_eq!(paranoid::SLOT_OFFSET, (1 + 15 + 2 + 5) * 8);
}

#[test]
fn disjoint_ist_spans_are_accepted() {
    let spans = [
        (0x0040_0000, 0x0040_4000),
        (0x0040_4000, 0x0040_8000),
        (0x0050_0000, 0x0050_4000),
    ];
    assert_eq!(paranoid::check_spans(&spans), Ok(()));
}

#[test]
fn overlapping_ist_spans_are_refused() {
    let spans = [(0x0040_0000, 0x0040_4000), (0x0040_3000, 0x0040_7000)];
    assert_eq!(
        paranoid::check_spans(&spans),
        Err(ParanoidError::IstOverlap)
    );
    // Deux CPU partageant la même pile NMI.
    let shared = [(0x0040_0000, 0x0040_4000), (0x0040_0000, 0x0040_4000)];
    assert_eq!(
        paranoid::check_spans(&shared),
        Err(ParanoidError::IstOverlap)
    );
}

#[test]
fn an_invalid_span_in_the_list_is_refused() {
    let spans = [(0x0040_0000, 0x0040_4000), (0, 0x4000)];
    assert_eq!(paranoid::check_spans(&spans), Err(ParanoidError::IstSpan));
}

// --- Portes de l'IDT -----------------------------------------------------------

#[test]
fn a_gate_is_decoded_like_the_kernel_writes_it() {
    let (low, high) = gate(0xFFFF_8000_1234_5678, 0x08, 0x8E02);
    let decoded = paranoid::decode_gate(low, high);
    assert!(decoded.present);
    assert_eq!(decoded.dpl, 0);
    assert_eq!(decoded.kind, 0xE);
    assert_eq!(decoded.ist, 2);
    assert_eq!(decoded.selector, 0x08);
    assert_eq!(decoded.handler, 0xFFFF_8000_1234_5678);
}

#[test]
fn the_expected_paranoid_gate_is_accepted() {
    let (low, high) = gate(0x0010_2000, 0x08, 0x8E00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Ok(())
    );
}

#[test]
fn a_gate_without_its_ist_is_refused() {
    let (low, high) = gate(0x0010_2000, 0x08, 0x8E00);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::GateIst)
    );
}

#[test]
fn a_gate_reachable_from_ring_3_or_not_present_is_refused() {
    let (low, high) = gate(0x0010_2000, 0x08, 0xEE00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::Gate)
    );
    let (low, high) = gate(0x0010_2000, 0x08, 0x0E00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::Gate)
    );
}

#[test]
fn a_trap_gate_is_refused() {
    // Une porte de trappe (0xF) laisserait IF actif dans le gestionnaire.
    let (low, high) = gate(0x0010_2000, 0x08, 0x8F00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::Gate)
    );
}

#[test]
fn a_gate_to_another_handler_or_selector_is_refused() {
    let (low, high) = gate(0x0010_3000, 0x08, 0x8E00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::GateTarget)
    );
    let (low, high) = gate(0x0010_2000, 0x10, 0x8E00 | paranoid::IST_NMI);
    assert_eq!(
        paranoid::check_gate(low, high, 0x08, 0x0010_2000, paranoid::IST_NMI),
        Err(ParanoidError::GateTarget)
    );
}

// --- Observations de la qualification ------------------------------------------

const AREA: u64 = 0x0012_3440;
const USER_GS: u64 = 0x0000_0080_0010_1000;
const IST: (u64, u64) = (0x0040_0000, 0x0040_4000);

fn expected(entry_gs: u64) -> Expected {
    Expected {
        kernel_gs: AREA,
        entry_gs,
        index: 2,
        ist: IST,
    }
}

fn observed(entry_gs: u64) -> Observation {
    Observation {
        entry_gs,
        dispatch_gs: AREA,
        dispatch_index: 2,
        rsp: IST.1 - 0x200,
    }
}

#[test]
fn an_entry_in_the_swapgs_window_is_accepted_with_the_kernel_gs() {
    assert_eq!(
        paranoid::check_observation(&observed(USER_GS), &expected(USER_GS)),
        Ok(())
    );
    assert!(paranoid::replaced(&observed(USER_GS), AREA));
}

#[test]
fn an_entry_from_kernel_mode_keeps_the_kernel_gs() {
    assert_eq!(
        paranoid::check_observation(&observed(AREA), &expected(AREA)),
        Ok(())
    );
    assert!(!paranoid::replaced(&observed(AREA), AREA));
}

#[test]
fn an_entry_that_did_not_see_the_expected_gs_is_refused() {
    // La qualification n'a pas créé la condition attendue (fenêtre manquée).
    assert_eq!(
        paranoid::check_observation(&observed(AREA), &expected(USER_GS)),
        Err(ParanoidError::EntryGs)
    );
}

#[test]
fn a_dispatch_with_the_user_gs_is_refused() {
    let mut observation = observed(USER_GS);
    observation.dispatch_gs = USER_GS;
    assert_eq!(
        paranoid::check_observation(&observation, &expected(USER_GS)),
        Err(ParanoidError::DispatchGs)
    );
}

#[test]
fn a_dispatch_reading_another_cpu_index_is_refused() {
    let mut observation = observed(USER_GS);
    observation.dispatch_index = 1;
    assert_eq!(
        paranoid::check_observation(&observation, &expected(USER_GS)),
        Err(ParanoidError::Index)
    );
}

#[test]
fn a_dispatch_outside_its_ist_is_refused() {
    let mut observation = observed(USER_GS);
    observation.rsp = IST.1 + 8;
    assert_eq!(
        paranoid::check_observation(&observation, &expected(USER_GS)),
        Err(ParanoidError::Stack)
    );
    observation.rsp = IST.0;
    assert_eq!(
        paranoid::check_observation(&observation, &expected(USER_GS)),
        Err(ParanoidError::Stack)
    );
}

// --- CR4.MCE -------------------------------------------------------------------

const CPUID_EDX_MCE: u32 = 1 << 7;
const CR4_MCE: u64 = 1 << 6;

#[test]
fn machine_check_must_be_enabled_when_supported() {
    assert_eq!(paranoid::check_mce(CR4_MCE, CPUID_EDX_MCE), Ok(()));
    assert_eq!(
        paranoid::check_mce(0, CPUID_EDX_MCE),
        Err(ParanoidError::MceDisabled)
    );
}

#[test]
fn machine_check_is_not_required_without_cpu_support() {
    assert_eq!(paranoid::check_mce(0, 0), Ok(()));
}

#[test]
fn every_error_has_a_message() {
    for error in [
        ParanoidError::IstSpan,
        ParanoidError::IstOverlap,
        ParanoidError::Gate,
        ParanoidError::GateIst,
        ParanoidError::GateTarget,
        ParanoidError::EntryGs,
        ParanoidError::DispatchGs,
        ParanoidError::Index,
        ParanoidError::Stack,
        ParanoidError::MceDisabled,
    ] {
        assert!(!error.as_str().is_empty());
    }
}
