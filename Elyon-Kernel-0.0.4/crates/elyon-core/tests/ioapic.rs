//! Tests hôte de la logique IOAPIC : décodage des registres, cohérence des
//! contrôleurs, plan de routage legacy (ISO), encodage des redirections et
//! validation de la fenêtre MMIO.

use elyon_abi::{MemoryRegion, MemoryRegionKind};
use elyon_core::acpi::{InterruptSourceOverride, IoApicDescriptor};
use elyon_core::ioapic::{
    self, IoApicError, IoApicState, LEGACY_VECTOR_BASE, REDIR_DELIVERY_STATUS, REDIR_MASKED,
    REDIR_POLARITY_LOW, REDIR_REMOTE_IRR, REDIR_TRIGGER_LEVEL,
};

const IDENTITY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

/// Registre version QEMU : version 0x20, dernière entrée 23 (24 redirections).
const QEMU_VERSION_RAW: u32 = 0x0017_0020;

fn descriptor(id: u8, address: u64, gsi_base: u32) -> IoApicDescriptor {
    IoApicDescriptor {
        id,
        address,
        gsi_base,
    }
}

fn controller(id: u8, address: u64, gsi_base: u32) -> IoApicState {
    ioapic::decode_controller(
        &descriptor(id, address, gsi_base),
        (id as u32) << 24,
        QEMU_VERSION_RAW,
        0,
    )
    .unwrap()
}

fn iso(source: u8, gsi: u32, flags: u16) -> InterruptSourceOverride {
    InterruptSourceOverride {
        bus: 0,
        source,
        gsi,
        flags,
    }
}

/// Les cinq ISO publiés par QEMU q35.
fn qemu_isos() -> Vec<InterruptSourceOverride> {
    vec![
        iso(0, 2, 0),
        iso(5, 5, 0xD),
        iso(9, 9, 0xD),
        iso(10, 10, 0xD),
        iso(11, 11, 0xD),
    ]
}

fn region(start: u64, pages: u64, kind: MemoryRegionKind) -> MemoryRegion {
    MemoryRegion {
        start,
        pages,
        kind,
        ..MemoryRegion::EMPTY
    }
}

// --- Décodage d'un contrôleur ---------------------------------------------------

#[test]
fn qemu_controller_is_decoded() {
    let state =
        ioapic::decode_controller(&descriptor(0, 0xFEC0_0000, 0), 0, QEMU_VERSION_RAW, 0).unwrap();
    assert_eq!(state.redirection_entries, 24);
    assert_eq!(state.version, 0x20);
    assert_eq!(state.gsi_end_exclusive(), 24);
    assert!(state.id_matches());
}

#[test]
fn hardware_id_uses_four_bits() {
    let state = ioapic::decode_controller(
        &descriptor(0, 0xFEC0_0000, 0),
        0xFF00_0000,
        QEMU_VERSION_RAW,
        0x0300_0000,
    )
    .unwrap();
    assert_eq!(state.hw_id, 0x0F);
    assert_eq!(state.arbitration_id, 3);
}

#[test]
fn vmware_id_divergence_is_diagnostic_only() {
    let state =
        ioapic::decode_controller(&descriptor(4, 0xFEC0_0000, 0), 0, QEMU_VERSION_RAW, 0).unwrap();
    assert_eq!((state.madt_id, state.hw_id), (4, 0));
    assert!(!state.id_matches());
}

#[test]
fn too_many_redirections_are_rejected() {
    let result = ioapic::decode_controller(&descriptor(0, 0xFEC0_0000, 0), 0, 0x00FF_0020, 0);
    assert_eq!(result.err(), Some(IoApicError::RedirectionCountInvalid));
}

#[test]
fn gsi_range_overflow_is_rejected() {
    let result = ioapic::decode_controller(
        &descriptor(0, 0xFEC0_0000, u32::MAX - 5),
        0,
        QEMU_VERSION_RAW,
        0,
    );
    assert_eq!(result.err(), Some(IoApicError::GsiRangeOverflow));
}

// --- Cohérence entre contrôleurs ---------------------------------------------------

#[test]
fn duplicate_madt_id_is_rejected() {
    let first = controller(0, 0xFEC0_0000, 0);
    let second = controller(0, 0xFEC0_1000, 24);
    assert_eq!(
        ioapic::check_new_controller(&[first], &second).err(),
        Some(IoApicError::DuplicateMadtId)
    );
}

#[test]
fn duplicate_address_is_rejected() {
    let first = controller(0, 0xFEC0_0000, 0);
    let second = controller(1, 0xFEC0_0000, 24);
    assert_eq!(
        ioapic::check_new_controller(&[first], &second).err(),
        Some(IoApicError::DuplicateControllerAddress)
    );
}

#[test]
fn overlapping_gsi_ranges_are_rejected() {
    let first = controller(0, 0xFEC0_0000, 0);
    let second = controller(1, 0xFEC0_1000, 23);
    assert_eq!(
        ioapic::check_new_controller(&[first], &second).err(),
        Some(IoApicError::GsiRangeOverlap)
    );
}

#[test]
fn adjacent_gsi_ranges_are_accepted() {
    let first = controller(0, 0xFEC0_0000, 0);
    let second = controller(1, 0xFEC0_1000, 24);
    assert!(ioapic::check_new_controller(&[first], &second).is_ok());
}

// --- Plan de routage legacy ----------------------------------------------------------

#[test]
fn no_controller_is_rejected() {
    assert_eq!(
        ioapic::plan_legacy_routes(&[], &[]).err(),
        Some(IoApicError::NoController)
    );
}

#[test]
fn identity_routes_without_overrides() {
    let plan = ioapic::plan_legacy_routes(&[], &[controller(0, 0xFEC0_0000, 0)]).unwrap();
    assert_eq!(plan.routes().len(), 16);
    assert_eq!(plan.overrides_applied, 0);
    assert_eq!(plan.shadowed_legacy_irqs, 0);
    for (irq, route) in plan.routes().iter().enumerate() {
        assert_eq!(route.irq as usize, irq);
        assert_eq!(route.gsi as usize, irq);
        assert_eq!(route.vector, LEGACY_VECTOR_BASE + irq as u8);
        assert_eq!((route.controller, route.redirection_index), (0, irq as u32));
        assert!(route.masked && !route.overridden);
        assert!(!route.active_low && !route.level_triggered);
    }
}

#[test]
fn qemu_overrides_match_ci_summary() {
    let plan = ioapic::plan_legacy_routes(&qemu_isos(), &[controller(0, 0xFEC0_0000, 0)]).unwrap();
    // Valeurs du gate CI : legacy_routes=15 overrides_applied=5 shadowed=1.
    assert_eq!(plan.routes().len(), 15);
    assert_eq!(plan.overrides_applied, 5);
    assert_eq!(plan.shadowed_legacy_irqs, 1);

    let timer = plan.routes()[0];
    assert_eq!((timer.irq, timer.gsi, timer.overridden), (0, 2, true));
    assert!(plan.routes().iter().all(|route| route.irq != 2));

    let sci = plan.routes().iter().find(|route| route.irq == 9).unwrap();
    assert!(!sci.active_low && sci.level_triggered);
}

#[test]
fn active_low_level_override_is_decoded() {
    let plan =
        ioapic::plan_legacy_routes(&[iso(9, 9, 0xF)], &[controller(0, 0xFEC0_0000, 0)]).unwrap();
    let sci = plan.routes()[9];
    assert!(sci.active_low && sci.level_triggered);
}

#[test]
fn reserved_polarity_is_rejected() {
    let result = ioapic::plan_legacy_routes(&[iso(9, 9, 0x2)], &[controller(0, 0xFEC0_0000, 0)]);
    assert_eq!(result.err(), Some(IoApicError::InvalidOverrideFlags));
}

#[test]
fn reserved_trigger_mode_is_rejected() {
    let result = ioapic::plan_legacy_routes(&[iso(9, 9, 0x8)], &[controller(0, 0xFEC0_0000, 0)]);
    assert_eq!(result.err(), Some(IoApicError::InvalidOverrideFlags));
}

#[test]
fn non_isa_override_bus_is_rejected() {
    let mut pci = iso(9, 9, 0);
    pci.bus = 1;
    let result = ioapic::plan_legacy_routes(&[pci], &[controller(0, 0xFEC0_0000, 0)]);
    assert_eq!(result.err(), Some(IoApicError::UnsupportedOverrideBus));
}

#[test]
fn duplicate_override_is_rejected() {
    let result = ioapic::plan_legacy_routes(
        &[iso(9, 9, 0), iso(9, 20, 0)],
        &[controller(0, 0xFEC0_0000, 0)],
    );
    assert_eq!(result.err(), Some(IoApicError::DuplicateOverride));
}

#[test]
fn two_irqs_on_same_gsi_are_rejected() {
    let result = ioapic::plan_legacy_routes(
        &[iso(3, 20, 0), iso(4, 20, 0)],
        &[controller(0, 0xFEC0_0000, 0)],
    );
    assert_eq!(result.err(), Some(IoApicError::DuplicateRoute));
}

#[test]
fn gsi_outside_every_controller_is_rejected() {
    let result = ioapic::plan_legacy_routes(&[iso(1, 50, 0)], &[controller(0, 0xFEC0_0000, 0)]);
    assert_eq!(result.err(), Some(IoApicError::RouteOutsideIoApic));
}

#[test]
fn route_targets_the_controller_owning_the_gsi() {
    let controllers = [
        controller(0, 0xFEC0_0000, 0),
        controller(1, 0xFEC0_1000, 24),
    ];
    let plan = ioapic::plan_legacy_routes(&[iso(3, 30, 0)], &controllers).unwrap();
    let route = plan.routes()[3];
    assert_eq!((route.controller, route.redirection_index), (1, 6));
}

// --- Encodage des redirections ----------------------------------------------------------

#[test]
fn redirection_registers_follow_the_table_layout() {
    assert_eq!(ioapic::redirection_registers(0), (0x10, 0x11));
    assert_eq!(ioapic::redirection_registers(23), (0x3E, 0x3F));
}

#[test]
fn edge_high_route_is_encoded_masked() {
    let plan = ioapic::plan_legacy_routes(&[], &[controller(0, 0xFEC0_0000, 0)]).unwrap();
    let (low, high) = ioapic::encode_route(&plan.routes()[1], 0);
    assert_eq!(low, 0x21 | REDIR_MASKED);
    assert_eq!(high, 0);
}

#[test]
fn level_low_route_is_encoded_with_destination() {
    let plan =
        ioapic::plan_legacy_routes(&[iso(9, 9, 0xF)], &[controller(0, 0xFEC0_0000, 0)]).unwrap();
    let (low, high) = ioapic::encode_route(&plan.routes()[9], 3);
    assert_eq!(
        low,
        0x29 | REDIR_MASKED | REDIR_POLARITY_LOW | REDIR_TRIGGER_LEVEL
    );
    assert_eq!(high, 3 << 24);
}

#[test]
fn read_back_ignores_read_only_status_bits() {
    let expected = (0x21 | REDIR_MASKED, 0);
    let read = (
        expected.0 | REDIR_DELIVERY_STATUS | REDIR_REMOTE_IRR,
        0x0000_00AB,
    );
    assert!(ioapic::read_back_matches(expected, read));
}

#[test]
fn read_back_detects_wrong_vector_or_destination() {
    let expected = (0x21 | REDIR_MASKED, 1 << 24);
    assert!(!ioapic::read_back_matches(
        expected,
        (0x22 | REDIR_MASKED, 1 << 24)
    ));
    assert!(!ioapic::read_back_matches(expected, (0x21, 1 << 24)));
    assert!(!ioapic::read_back_matches(
        expected,
        (0x21 | REDIR_MASKED, 2 << 24)
    ));
}

// --- Fenêtre MMIO (H4) ---------------------------------------------------------------------

#[test]
fn mmio_outside_memory_map_is_accepted() {
    let regions = [region(0x10_0000, 0x1000, MemoryRegionKind::Usable)];
    assert!(ioapic::validate_mmio_window(0xFEC0_0000, IDENTITY_LIMIT, &regions, &[]).is_ok());
}

#[test]
fn mmio_inside_reserved_or_mmio_region_is_accepted() {
    let regions = [
        region(0xFEC0_0000, 1, MemoryRegionKind::Mmio),
        region(0xFEC0_1000, 1, MemoryRegionKind::Reserved),
    ];
    assert!(ioapic::validate_mmio_window(0xFEC0_0000, IDENTITY_LIMIT, &regions, &[]).is_ok());
    assert!(ioapic::validate_mmio_window(0xFEC0_1000, IDENTITY_LIMIT, &regions, &[]).is_ok());
}

#[test]
fn null_mmio_is_rejected() {
    assert_eq!(
        ioapic::validate_mmio_window(0, IDENTITY_LIMIT, &[], &[]).err(),
        Some(IoApicError::InvalidAddress)
    );
}

#[test]
fn mmio_beyond_identity_map_is_rejected() {
    assert_eq!(
        ioapic::validate_mmio_window(IDENTITY_LIMIT, IDENTITY_LIMIT, &[], &[]).err(),
        Some(IoApicError::MmioOutsideIdentityMap)
    );
}

#[test]
fn misaligned_mmio_is_rejected() {
    assert_eq!(
        ioapic::validate_mmio_window(0xFEC0_0004, IDENTITY_LIMIT, &[], &[]).err(),
        Some(IoApicError::MmioMisaligned)
    );
}

#[test]
fn mmio_in_ram_is_rejected() {
    for kind in [
        MemoryRegionKind::Usable,
        MemoryRegionKind::Reclaimable,
        MemoryRegionKind::Bootstrap,
    ] {
        let regions = [region(0x0100_0000, 0x100, kind)];
        assert_eq!(
            ioapic::validate_mmio_window(0x0100_8000, IDENTITY_LIMIT, &regions, &[]).err(),
            Some(IoApicError::MmioOverlapsRam),
            "{kind:?}"
        );
    }
}

#[test]
fn mmio_over_kernel_image_is_rejected() {
    let protected = [(0x20_0000, 0x40_0000)];
    assert_eq!(
        ioapic::validate_mmio_window(0x30_0000, IDENTITY_LIMIT, &[], &protected).err(),
        Some(IoApicError::MmioOverlapsKernel)
    );
}
