//! Tests hôte du parseur ACPI (RSDP, RSDT/XSDT, SDT, MADT).
//!
//! Les tables sont construites en mémoire avec des checksums calculés, puis
//! exposées au parseur via une fausse mémoire physique.

use elyon_core::acpi::{self, AcpiError, PhysMemory, RootKind};

// --- Fausse mémoire physique -------------------------------------------------

struct FakeMemory {
    regions: Vec<(u64, Vec<u8>)>,
}

impl FakeMemory {
    fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    fn place(mut self, address: u64, bytes: Vec<u8>) -> Self {
        self.regions.push((address, bytes));
        self
    }
}

impl PhysMemory for FakeMemory {
    fn bytes(&self, address: u64, length: usize) -> Result<&[u8], AcpiError> {
        for (base, bytes) in &self.regions {
            let start = match address.checked_sub(*base) {
                Some(start) => start as usize,
                None => continue,
            };
            if let Some(slice) = bytes.get(start..start + length) {
                return Ok(slice);
            }
        }
        Err(AcpiError::AddressOutsideMappedRange)
    }
}

// --- Construction des tables -------------------------------------------------

const RSDP_ADDR: u64 = 0xE_0000;
const ROOT_ADDR: u64 = 0x1000;
const MADT_ADDR: u64 = 0x2000;
const FACP_ADDR: u64 = 0x3000;

fn fix_checksum(bytes: &mut [u8], at: usize) {
    bytes[at] = 0;
    let sum = bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    bytes[at] = 0u8.wrapping_sub(sum);
}

fn sdt(signature: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut table = vec![0u8; 36];
    table[..4].copy_from_slice(signature);
    table[8] = 1;
    table[10..16].copy_from_slice(b"ELYON ");
    table.extend_from_slice(payload);
    let length = table.len() as u32;
    table[4..8].copy_from_slice(&length.to_le_bytes());
    fix_checksum(&mut table, 9);
    table
}

fn rsdp_v1(rsdt: u32) -> Vec<u8> {
    let mut rsdp = vec![0u8; 20];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[9..15].copy_from_slice(b"ELYON ");
    rsdp[15] = 0;
    rsdp[16..20].copy_from_slice(&rsdt.to_le_bytes());
    fix_checksum(&mut rsdp, 8);
    rsdp
}

fn rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
    let mut rsdp = vec![0u8; 36];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[9..15].copy_from_slice(b"ELYON ");
    rsdp[15] = 2;
    rsdp[16..20].copy_from_slice(&rsdt.to_le_bytes());
    rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
    rsdp[24..32].copy_from_slice(&xsdt.to_le_bytes());
    fix_checksum(&mut rsdp[..20], 8);
    fix_checksum(&mut rsdp, 32);
    rsdp
}

fn rsdt(tables: &[u32]) -> Vec<u8> {
    let payload: Vec<u8> = tables.iter().flat_map(|a| a.to_le_bytes()).collect();
    sdt(b"RSDT", &payload)
}

fn xsdt(tables: &[u64]) -> Vec<u8> {
    let payload: Vec<u8> = tables.iter().flat_map(|a| a.to_le_bytes()).collect();
    sdt(b"XSDT", &payload)
}

fn madt(lapic_address: u32, flags: u32, entries: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&lapic_address.to_le_bytes());
    payload.extend_from_slice(&flags.to_le_bytes());
    for entry in entries {
        payload.extend_from_slice(entry);
    }
    sdt(b"APIC", &payload)
}

fn lapic(uid: u8, apic_id: u8, flags: u32) -> Vec<u8> {
    let mut e = vec![0, 8, uid, apic_id];
    e.extend_from_slice(&flags.to_le_bytes());
    e
}

fn ioapic(id: u8, address: u32, gsi_base: u32) -> Vec<u8> {
    let mut e = vec![1, 12, id, 0];
    e.extend_from_slice(&address.to_le_bytes());
    e.extend_from_slice(&gsi_base.to_le_bytes());
    e
}

fn iso(source: u8, gsi: u32, flags: u16) -> Vec<u8> {
    let mut e = vec![2, 10, 0, source];
    e.extend_from_slice(&gsi.to_le_bytes());
    e.extend_from_slice(&flags.to_le_bytes());
    e
}

fn nmi_source(flags: u16, gsi: u32) -> Vec<u8> {
    let mut e = vec![3, 8];
    e.extend_from_slice(&flags.to_le_bytes());
    e.extend_from_slice(&gsi.to_le_bytes());
    e
}

fn lapic_nmi(uid: u8, flags: u16, lint: u8) -> Vec<u8> {
    let mut e = vec![4, 6, uid];
    e.extend_from_slice(&flags.to_le_bytes());
    e.push(lint);
    e
}

fn lapic_override(address: u64) -> Vec<u8> {
    let mut e = vec![5, 12, 0, 0];
    e.extend_from_slice(&address.to_le_bytes());
    e
}

fn x2apic(apic_id: u32, flags: u32, uid: u32) -> Vec<u8> {
    let mut e = vec![9, 16, 0, 0];
    e.extend_from_slice(&apic_id.to_le_bytes());
    e.extend_from_slice(&flags.to_le_bytes());
    e.extend_from_slice(&uid.to_le_bytes());
    e
}

fn x2apic_nmi(flags: u16, uid: u32, lint: u8) -> Vec<u8> {
    let mut e = vec![10, 12];
    e.extend_from_slice(&flags.to_le_bytes());
    e.extend_from_slice(&uid.to_le_bytes());
    e.extend_from_slice(&[lint, 0, 0, 0]);
    e
}

/// MADT proche de celle de QEMU q35 en 4 vCPU.
fn qemu_madt() -> Vec<u8> {
    madt(
        0xFEE0_0000,
        1,
        &[
            lapic(0, 0, 1),
            lapic(1, 1, 1),
            lapic(2, 2, 1),
            lapic(3, 3, 1),
            ioapic(0, 0xFEC0_0000, 0),
            iso(0, 2, 0),
            iso(5, 5, 0xD),
            iso(9, 9, 0xD),
            iso(10, 10, 0xD),
            iso(11, 11, 0xD),
            lapic_nmi(0xFF, 0, 1),
        ],
    )
}

/// Machine XSDT -> [FACP, MADT], RSDP v2.
fn machine_with_madt(madt: Vec<u8>) -> FakeMemory {
    FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[FACP_ADDR, MADT_ADDR]))
        .place(FACP_ADDR, sdt(b"FACP", &[0u8; 8]))
        .place(MADT_ADDR, madt)
}

fn parse_err(memory: &FakeMemory) -> AcpiError {
    match acpi::parse(memory, RSDP_ADDR) {
        Ok(_) => panic!("parse should have failed"),
        Err(error) => error,
    }
}

// --- RSDP ---------------------------------------------------------------------

#[test]
fn rsdp_v2_prefers_xsdt() {
    let memory = machine_with_madt(qemu_madt());
    let topology = acpi::parse(&memory, RSDP_ADDR).unwrap();
    assert_eq!(topology.rsdp_revision, 2);
    assert_eq!(topology.root_kind, RootKind::Xsdt);
    assert_eq!(topology.root_address, ROOT_ADDR);
    assert_eq!(topology.root_entries, 2);
    assert_eq!(topology.validated_table_count, 2);
}

#[test]
fn rsdp_v1_uses_rsdt() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v1(ROOT_ADDR as u32))
        .place(ROOT_ADDR, rsdt(&[MADT_ADDR as u32]))
        .place(MADT_ADDR, qemu_madt());
    let topology = acpi::parse(&memory, RSDP_ADDR).unwrap();
    assert_eq!(topology.rsdp_revision, 0);
    assert_eq!(topology.root_kind, RootKind::Rsdt);
    assert_eq!(topology.root_entries, 1);
}

#[test]
fn rsdp_v2_with_null_xsdt_falls_back_to_rsdt() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(ROOT_ADDR as u32, 0))
        .place(ROOT_ADDR, rsdt(&[MADT_ADDR as u32]))
        .place(MADT_ADDR, qemu_madt());
    let topology = acpi::parse(&memory, RSDP_ADDR).unwrap();
    assert_eq!(topology.root_kind, RootKind::Rsdt);
}

#[test]
fn null_rsdp_is_missing() {
    let memory = machine_with_madt(qemu_madt());
    assert_eq!(acpi::parse(&memory, 0).err(), Some(AcpiError::MissingRsdp));
}

#[test]
fn rsdp_bad_signature_is_rejected() {
    let mut rsdp = rsdp_v2(0, ROOT_ADDR);
    rsdp[0] = b'X';
    let memory = machine_with_madt(qemu_madt()).place(0xF_0000, rsdp);
    assert_eq!(
        acpi::parse(&memory, 0xF_0000).err(),
        Some(AcpiError::InvalidRsdpSignature)
    );
}

#[test]
fn rsdp_bad_v1_checksum_is_rejected() {
    let mut rsdp = rsdp_v1(ROOT_ADDR as u32);
    rsdp[8] = rsdp[8].wrapping_add(1);
    let memory = machine_with_madt(qemu_madt()).place(0xF_0000, rsdp);
    assert_eq!(
        acpi::parse(&memory, 0xF_0000).err(),
        Some(AcpiError::InvalidRsdpChecksum)
    );
}

#[test]
fn rsdp_bad_extended_checksum_is_rejected() {
    let mut rsdp = rsdp_v2(0, ROOT_ADDR);
    rsdp[32] = rsdp[32].wrapping_add(1);
    let memory = machine_with_madt(qemu_madt()).place(0xF_0000, rsdp);
    assert_eq!(
        acpi::parse(&memory, 0xF_0000).err(),
        Some(AcpiError::InvalidRsdpChecksum)
    );
}

#[test]
fn rsdp_v2_length_out_of_range_is_rejected() {
    let mut rsdp = rsdp_v2(0, ROOT_ADDR);
    rsdp[20..24].copy_from_slice(&20u32.to_le_bytes());
    fix_checksum(&mut rsdp[..20], 8);
    fix_checksum(&mut rsdp, 32);
    let memory = machine_with_madt(qemu_madt()).place(0xF_0000, rsdp);
    assert_eq!(
        acpi::parse(&memory, 0xF_0000).err(),
        Some(AcpiError::InvalidRsdpLength)
    );
}

#[test]
fn rsdp_outside_memory_is_rejected() {
    let memory = machine_with_madt(qemu_madt());
    assert_eq!(
        acpi::parse(&memory, 0xDEAD_0000).err(),
        Some(AcpiError::AddressOutsideMappedRange)
    );
}

// --- RSDT / XSDT ---------------------------------------------------------------

#[test]
fn root_with_wrong_signature_is_rejected() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, rsdt(&[MADT_ADDR as u32]))
        .place(MADT_ADDR, qemu_madt());
    assert_eq!(parse_err(&memory), AcpiError::InvalidRootSignature);
}

#[test]
fn root_with_misaligned_payload_is_rejected() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, sdt(b"XSDT", &[0u8; 12]))
        .place(MADT_ADDR, qemu_madt());
    assert_eq!(parse_err(&memory), AcpiError::InvalidRootPayload);
}

#[test]
fn null_table_address_is_rejected() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[0, MADT_ADDR]))
        .place(MADT_ADDR, qemu_madt());
    assert_eq!(parse_err(&memory), AcpiError::NullTableAddress);
}

#[test]
fn sdt_shorter_than_header_is_rejected() {
    let mut facp = sdt(b"FACP", &[]);
    facp[4..8].copy_from_slice(&8u32.to_le_bytes());
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[FACP_ADDR, MADT_ADDR]))
        .place(FACP_ADDR, facp)
        .place(MADT_ADDR, qemu_madt());
    assert_eq!(parse_err(&memory), AcpiError::InvalidSdtLength);
}

#[test]
fn missing_madt_is_rejected() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[FACP_ADDR]))
        .place(FACP_ADDR, sdt(b"FACP", &[0u8; 8]));
    assert_eq!(parse_err(&memory), AcpiError::MadtNotFound);
}

#[test]
fn duplicate_madt_is_rejected() {
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[MADT_ADDR, MADT_ADDR]))
        .place(MADT_ADDR, qemu_madt());
    assert_eq!(parse_err(&memory), AcpiError::DuplicateMadt);
}

#[test]
fn madt_with_bad_checksum_is_fatal() {
    let mut table = qemu_madt();
    table[9] = table[9].wrapping_add(1);
    let memory = machine_with_madt(table);
    assert_eq!(parse_err(&memory), AcpiError::InvalidSdtChecksum);
}

// --- MADT ------------------------------------------------------------------------

#[test]
fn qemu_madt_topology() {
    let memory = machine_with_madt(qemu_madt());
    let topology = acpi::parse(&memory, RSDP_ADDR).unwrap();

    assert_eq!(topology.madt_address, MADT_ADDR);
    assert_eq!(topology.local_apic_address, 0xFEE0_0000);
    assert_eq!(topology.madt_flags, 1);
    assert_eq!(topology.cpu_count(), 4);
    assert_eq!(topology.usable_cpu_count(), 4);
    let ids: Vec<u32> = topology.cpus().iter().map(|c| c.apic_id).collect();
    assert_eq!(ids, [0, 1, 2, 3]);

    assert_eq!(topology.io_apic_count(), 1);
    let io = topology.io_apics()[0];
    assert_eq!((io.id, io.address, io.gsi_base), (0, 0xFEC0_0000, 0));

    assert_eq!(topology.iso_count(), 5);
    let first = topology.isos()[0];
    assert_eq!(
        (first.bus, first.source, first.gsi, first.flags),
        (0, 0, 2, 0)
    );

    assert_eq!(topology.nmi_count(), 1);
}

#[test]
fn vmware_ioapic_id_is_reported_verbatim() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[lapic(0, 0, 1), lapic(1, 1, 1), ioapic(4, 0xFEC0_0000, 0)],
    );
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.io_apics()[0].id, 4);
}

#[test]
fn madt_shorter_than_fixed_part_is_rejected() {
    let table = sdt(b"APIC", &[0u8; 4]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtLength
    );
}

#[test]
fn madt_entry_with_zero_length_is_rejected() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 0, 1), vec![0, 0, 0, 0]]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn madt_entry_with_length_one_is_rejected() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 0, 1), vec![0, 1]]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn madt_entry_overflowing_table_is_rejected() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 0, 1), vec![1, 12, 0, 0]]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn madt_truncated_entry_header_is_rejected() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 0, 1), vec![0]]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn madt_entry_shorter_than_its_type_is_rejected() {
    let table = madt(0xFEE0_0000, 1, &[vec![1, 4, 0, 0]]);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn unknown_madt_entries_are_skipped() {
    let table = madt(0xFEE0_0000, 1, &[vec![0x7F, 4, 0, 0], lapic(0, 0, 1)]);
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 1);
}

#[test]
fn lapic_address_override_is_applied_once() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[lapic(0, 0, 1), lapic_override(0x1_FEE0_0000)],
    );
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.local_apic_address, 0x1_FEE0_0000);
}

#[test]
fn duplicate_lapic_address_override_is_rejected() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[lapic_override(0xFEE0_0000), lapic_override(0xFEE0_0000)],
    );
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::InvalidMadtEntry
    );
}

#[test]
fn x2apic_entries_are_parsed() {
    let table = madt(0xFEE0_0000, 1, &[x2apic(0x100, 1, 7)]);
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    let cpu = topology.cpus()[0];
    assert_eq!((cpu.apic_id, cpu.acpi_uid, cpu.x2apic), (0x100, 7, true));
}

#[test]
fn nmi_entries_of_all_types_are_parsed() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[
            nmi_source(0xD, 9),
            lapic_nmi(1, 5, 1),
            x2apic_nmi(5, 0x100, 0),
        ],
    );
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    let nmis = topology.nmis();
    assert_eq!(nmis.len(), 3);
    assert_eq!(
        (nmis[0].entry_type, nmis[0].gsi, nmis[0].flags),
        (3, 9, 0xD)
    );
    assert_eq!(
        (nmis[1].entry_type, nmis[1].acpi_uid, nmis[1].lint),
        (4, 1, 1)
    );
    assert_eq!(
        (nmis[2].entry_type, nmis[2].acpi_uid, nmis[2].lint),
        (10, 0x100, 0)
    );
}

#[test]
fn too_many_cpus_exceed_capacity() {
    let entries: Vec<Vec<u8>> = (0..=acpi::MAX_ACPI_CPUS as u32)
        .map(|id| x2apic(id, 1, id))
        .collect();
    let table = madt(0xFEE0_0000, 1, &entries);
    assert_eq!(
        parse_err(&machine_with_madt(table)),
        AcpiError::CapacityExceeded
    );
}

// --- Résilience (stabilisation 0.0.3) -------------------------------------------

#[test]
fn non_essential_table_with_bad_checksum_is_skipped() {
    let mut facp = sdt(b"FACP", &[0u8; 8]);
    facp[9] = facp[9].wrapping_add(1);
    let memory = FakeMemory::new()
        .place(RSDP_ADDR, rsdp_v2(0, ROOT_ADDR))
        .place(ROOT_ADDR, xsdt(&[FACP_ADDR, MADT_ADDR]))
        .place(FACP_ADDR, facp)
        .place(MADT_ADDR, qemu_madt());
    let topology = acpi::parse(&memory, RSDP_ADDR).unwrap();
    assert_eq!(topology.validated_table_count, 1);
    assert_eq!(topology.skipped_table_count, 1);
    assert_eq!(topology.cpu_count(), 4);
}

#[test]
fn x2apic_entry_duplicating_xapic_entry_is_ignored() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[lapic(0, 0, 1), lapic(1, 1, 1), x2apic(1, 1, 1)],
    );
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 2);
    assert!(topology.cpus().iter().all(|cpu| !cpu.x2apic));
}

#[test]
fn xapic_entry_duplicating_x2apic_entry_is_ignored() {
    let table = madt(0xFEE0_0000, 1, &[x2apic(0, 1, 0), lapic(0, 0, 1)]);
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 1);
    assert!(topology.cpus()[0].x2apic);
}

#[test]
fn duplicate_within_same_entry_type_is_kept_for_downstream_rejection() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 1, 1), lapic(1, 1, 1)]);
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 2);
}

#[test]
fn xapic_broadcast_id_is_ignored() {
    let table = madt(0xFEE0_0000, 1, &[lapic(0, 0, 1), lapic(1, 0xFF, 1)]);
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 1);
}

#[test]
fn online_capable_only_cpu_is_reported_but_not_usable() {
    let table = madt(
        0xFEE0_0000,
        1,
        &[lapic(0, 0, 1), lapic(1, 1, 2), lapic(2, 2, 0)],
    );
    let topology = acpi::parse(&machine_with_madt(table), RSDP_ADDR).unwrap();
    assert_eq!(topology.cpu_count(), 3);
    assert_eq!(topology.usable_cpu_count(), 1);
    assert!(topology.cpus()[1].online_capable());
    assert!(!topology.cpus()[1].usable());
}
