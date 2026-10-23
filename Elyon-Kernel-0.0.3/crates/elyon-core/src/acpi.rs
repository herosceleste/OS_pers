//! Parseur ACPI minimal : RSDP, RSDT/XSDT, en-têtes SDT et MADT.
//!
//! L'accès à la mémoire physique est fourni par l'appelant via [`PhysMemory`].

const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
const RSDP_V1_LENGTH: usize = 20;
const RSDP_V2_MIN_LENGTH: usize = 36;
const SDT_HEADER_LENGTH: usize = 36;
const MADT_FIXED_LENGTH: usize = 44;
const MAX_RSDP_LENGTH: usize = 4096;
const MAX_TABLE_LENGTH: usize = 16 * 1024 * 1024;

pub const MAX_ACPI_CPUS: usize = 256;
pub const MAX_IO_APICS: usize = 16;
pub const MAX_ISO_ENTRIES: usize = 64;
pub const MAX_NMI_ENTRIES: usize = 64;

const XAPIC_BROADCAST_ID: u32 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    Rsdt,
    Xsdt,
}

impl RootKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rsdt => "RSDT",
            Self::Xsdt => "XSDT",
        }
    }

    const fn signature(self) -> &'static [u8; 4] {
        match self {
            Self::Rsdt => b"RSDT",
            Self::Xsdt => b"XSDT",
        }
    }

    const fn entry_size(self) -> usize {
        match self {
            Self::Rsdt => 4,
            Self::Xsdt => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuDescriptor {
    pub acpi_uid: u32,
    pub apic_id: u32,
    pub flags: u32,
    pub x2apic: bool,
}

impl CpuDescriptor {
    const EMPTY: Self = Self {
        acpi_uid: 0,
        apic_id: 0,
        flags: 0,
        x2apic: false,
    };

    pub const fn enabled(self) -> bool {
        self.flags & 1 != 0
    }

    pub const fn online_capable(self) -> bool {
        self.flags & 2 != 0
    }

    /// Démarrable au boot : seul le bit Enabled compte. Un CPU seulement
    /// Online Capable est hot-pluggable et ne répondra pas à INIT/SIPI.
    pub const fn usable(self) -> bool {
        self.enabled()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApicDescriptor {
    pub id: u8,
    pub address: u64,
    pub gsi_base: u32,
}

impl IoApicDescriptor {
    const EMPTY: Self = Self {
        id: 0,
        address: 0,
        gsi_base: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptSourceOverride {
    pub bus: u8,
    pub source: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl InterruptSourceOverride {
    const EMPTY: Self = Self {
        bus: 0,
        source: 0,
        gsi: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmiDescriptor {
    pub entry_type: u8,
    pub acpi_uid: u32,
    pub gsi: u32,
    pub flags: u16,
    pub lint: u8,
}

impl NmiDescriptor {
    const EMPTY: Self = Self {
        entry_type: 0,
        acpi_uid: u32::MAX,
        gsi: 0,
        flags: 0,
        lint: 0,
    };
}

pub struct AcpiTopology {
    pub rsdp_revision: u8,
    pub root_kind: RootKind,
    pub root_address: u64,
    pub root_entries: usize,
    pub validated_table_count: usize,
    pub skipped_table_count: usize,
    pub madt_address: u64,
    pub local_apic_address: u64,
    pub madt_flags: u32,
    cpus: [CpuDescriptor; MAX_ACPI_CPUS],
    cpu_count: usize,
    io_apics: [IoApicDescriptor; MAX_IO_APICS],
    io_apic_count: usize,
    isos: [InterruptSourceOverride; MAX_ISO_ENTRIES],
    iso_count: usize,
    nmis: [NmiDescriptor; MAX_NMI_ENTRIES],
    nmi_count: usize,
}

impl AcpiTopology {
    const fn empty(revision: u8, root_kind: RootKind, root_address: u64) -> Self {
        Self {
            rsdp_revision: revision,
            root_kind,
            root_address,
            root_entries: 0,
            validated_table_count: 0,
            skipped_table_count: 0,
            madt_address: 0,
            local_apic_address: 0,
            madt_flags: 0,
            cpus: [CpuDescriptor::EMPTY; MAX_ACPI_CPUS],
            cpu_count: 0,
            io_apics: [IoApicDescriptor::EMPTY; MAX_IO_APICS],
            io_apic_count: 0,
            isos: [InterruptSourceOverride::EMPTY; MAX_ISO_ENTRIES],
            iso_count: 0,
            nmis: [NmiDescriptor::EMPTY; MAX_NMI_ENTRIES],
            nmi_count: 0,
        }
    }

    pub fn cpus(&self) -> &[CpuDescriptor] {
        &self.cpus[..self.cpu_count]
    }

    pub fn io_apics(&self) -> &[IoApicDescriptor] {
        &self.io_apics[..self.io_apic_count]
    }

    pub fn isos(&self) -> &[InterruptSourceOverride] {
        &self.isos[..self.iso_count]
    }

    pub fn nmis(&self) -> &[NmiDescriptor] {
        &self.nmis[..self.nmi_count]
    }

    pub const fn cpu_count(&self) -> usize {
        self.cpu_count
    }

    pub fn usable_cpu_count(&self) -> usize {
        self.cpus().iter().filter(|cpu| cpu.usable()).count()
    }

    pub const fn io_apic_count(&self) -> usize {
        self.io_apic_count
    }

    pub const fn iso_count(&self) -> usize {
        self.iso_count
    }

    pub const fn nmi_count(&self) -> usize {
        self.nmi_count
    }

    fn push_cpu(&mut self, cpu: CpuDescriptor) -> Result<(), AcpiError> {
        // 0xFF est l'ID de broadcast xAPIC : jamais un CPU réel.
        if !cpu.x2apic && cpu.apic_id == XAPIC_BROADCAST_ID {
            return Ok(());
        }
        // Un firmware peut décrire le même CPU en xAPIC et en x2APIC : on garde
        // la première description. Les doublons d'un même type restent et sont
        // rejetés en aval par `percpu`.
        if self
            .cpus()
            .iter()
            .any(|known| known.apic_id == cpu.apic_id && known.x2apic != cpu.x2apic)
        {
            return Ok(());
        }
        if self.cpu_count >= MAX_ACPI_CPUS {
            return Err(AcpiError::CapacityExceeded);
        }
        self.cpus[self.cpu_count] = cpu;
        self.cpu_count += 1;
        Ok(())
    }

    fn push_io_apic(&mut self, io_apic: IoApicDescriptor) -> Result<(), AcpiError> {
        if self.io_apic_count >= MAX_IO_APICS {
            return Err(AcpiError::CapacityExceeded);
        }
        self.io_apics[self.io_apic_count] = io_apic;
        self.io_apic_count += 1;
        Ok(())
    }

    fn push_iso(&mut self, iso: InterruptSourceOverride) -> Result<(), AcpiError> {
        if self.iso_count >= MAX_ISO_ENTRIES {
            return Err(AcpiError::CapacityExceeded);
        }
        self.isos[self.iso_count] = iso;
        self.iso_count += 1;
        Ok(())
    }

    fn push_nmi(&mut self, nmi: NmiDescriptor) -> Result<(), AcpiError> {
        if self.nmi_count >= MAX_NMI_ENTRIES {
            return Err(AcpiError::CapacityExceeded);
        }
        self.nmis[self.nmi_count] = nmi;
        self.nmi_count += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpiError {
    MissingRsdp,
    AddressOutsideMappedRange,
    InvalidRsdpSignature,
    InvalidRsdpChecksum,
    InvalidRsdpLength,
    InvalidSdtLength,
    InvalidSdtChecksum,
    InvalidRootSignature,
    InvalidRootPayload,
    NullTableAddress,
    MadtNotFound,
    DuplicateMadt,
    InvalidMadtLength,
    InvalidMadtEntry,
    CapacityExceeded,
}

impl AcpiError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingRsdp => "RSDP missing",
            Self::AddressOutsideMappedRange => "ACPI address outside current identity map",
            Self::InvalidRsdpSignature => "invalid RSDP signature",
            Self::InvalidRsdpChecksum => "invalid RSDP checksum",
            Self::InvalidRsdpLength => "invalid RSDP length",
            Self::InvalidSdtLength => "invalid ACPI SDT length",
            Self::InvalidSdtChecksum => "invalid ACPI SDT checksum",
            Self::InvalidRootSignature => "unexpected RSDT/XSDT signature",
            Self::InvalidRootPayload => "misaligned RSDT/XSDT payload",
            Self::NullTableAddress => "null ACPI table address",
            Self::MadtNotFound => "MADT not found",
            Self::DuplicateMadt => "multiple MADT tables",
            Self::InvalidMadtLength => "invalid MADT length",
            Self::InvalidMadtEntry => "invalid MADT entry",
            Self::CapacityExceeded => "ACPI topology capacity exceeded",
        }
    }
}

/// Vue en lecture seule sur la mémoire physique.
///
/// L'implémentation doit refuser toute plage nulle, non mappée ou qui déborde,
/// avec [`AcpiError::AddressOutsideMappedRange`].
pub trait PhysMemory {
    fn bytes(&self, address: u64, length: usize) -> Result<&[u8], AcpiError>;
}

struct RsdpInfo {
    revision: u8,
    root_kind: RootKind,
    root_address: u64,
}

pub fn parse<M: PhysMemory + ?Sized>(
    memory: &M,
    rsdp_address: u64,
) -> Result<AcpiTopology, AcpiError> {
    let rsdp = parse_rsdp(memory, rsdp_address)?;
    let root = read_sdt(memory, rsdp.root_address)?;

    if signature(root) != rsdp.root_kind.signature() {
        return Err(AcpiError::InvalidRootSignature);
    }

    let entry_size = rsdp.root_kind.entry_size();
    let payload_length = root.len() - SDT_HEADER_LENGTH;
    if !payload_length.is_multiple_of(entry_size) {
        return Err(AcpiError::InvalidRootPayload);
    }

    let entry_count = payload_length / entry_size;
    let mut topology = AcpiTopology::empty(rsdp.revision, rsdp.root_kind, rsdp.root_address);
    topology.root_entries = entry_count;

    let mut madt_seen = false;

    for index in 0..entry_count {
        let offset = SDT_HEADER_LENGTH + index * entry_size;
        let table_address = match rsdp.root_kind {
            RootKind::Rsdt => read_u32(root, offset)? as u64,
            RootKind::Xsdt => read_u64(root, offset)?,
        };

        if table_address == 0 {
            return Err(AcpiError::NullTableAddress);
        }

        let table = read_sdt_bytes(memory, table_address)?;
        let is_madt = signature(table) == b"APIC";

        // Seule la MADT est indispensable à Elyon : une autre table corrompue
        // est comptée puis ignorée, la MADT reste fail-closed.
        if !checksum_ok(table) {
            if is_madt {
                return Err(AcpiError::InvalidSdtChecksum);
            }
            topology.skipped_table_count += 1;
            continue;
        }
        topology.validated_table_count += 1;

        if is_madt {
            if madt_seen {
                return Err(AcpiError::DuplicateMadt);
            }
            parse_madt(table_address, table, &mut topology)?;
            madt_seen = true;
        }
    }

    if !madt_seen {
        return Err(AcpiError::MadtNotFound);
    }

    Ok(topology)
}

fn parse_rsdp<M: PhysMemory + ?Sized>(memory: &M, address: u64) -> Result<RsdpInfo, AcpiError> {
    if address == 0 {
        return Err(AcpiError::MissingRsdp);
    }

    let v1 = memory.bytes(address, RSDP_V1_LENGTH)?;
    if v1.get(..8) != Some(&RSDP_SIGNATURE[..]) {
        return Err(AcpiError::InvalidRsdpSignature);
    }
    if !checksum_ok(v1) {
        return Err(AcpiError::InvalidRsdpChecksum);
    }

    let revision = v1[15];
    let rsdt_address = read_u32(v1, 16)? as u64;

    if revision >= 2 {
        let v2_header = memory.bytes(address, RSDP_V2_MIN_LENGTH)?;
        let length = read_u32(v2_header, 20)? as usize;
        if !(RSDP_V2_MIN_LENGTH..=MAX_RSDP_LENGTH).contains(&length) {
            return Err(AcpiError::InvalidRsdpLength);
        }

        let full = memory.bytes(address, length)?;
        if !checksum_ok(full) {
            return Err(AcpiError::InvalidRsdpChecksum);
        }

        let xsdt_address = read_u64(full, 24)?;
        if xsdt_address != 0 {
            return Ok(RsdpInfo {
                revision,
                root_kind: RootKind::Xsdt,
                root_address: xsdt_address,
            });
        }
    }

    if rsdt_address == 0 {
        return Err(AcpiError::NullTableAddress);
    }

    Ok(RsdpInfo {
        revision,
        root_kind: RootKind::Rsdt,
        root_address: rsdt_address,
    })
}

fn read_sdt<M: PhysMemory + ?Sized>(memory: &M, address: u64) -> Result<&[u8], AcpiError> {
    let table = read_sdt_bytes(memory, address)?;
    if !checksum_ok(table) {
        return Err(AcpiError::InvalidSdtChecksum);
    }

    Ok(table)
}

/// Lit une SDT dont la longueur est validée, sans vérifier son checksum.
fn read_sdt_bytes<M: PhysMemory + ?Sized>(memory: &M, address: u64) -> Result<&[u8], AcpiError> {
    let header = memory.bytes(address, SDT_HEADER_LENGTH)?;
    let length = read_u32(header, 4)? as usize;

    if !(SDT_HEADER_LENGTH..=MAX_TABLE_LENGTH).contains(&length) {
        return Err(AcpiError::InvalidSdtLength);
    }

    memory.bytes(address, length)
}

fn parse_madt(address: u64, madt: &[u8], topology: &mut AcpiTopology) -> Result<(), AcpiError> {
    if madt.len() < MADT_FIXED_LENGTH {
        return Err(AcpiError::InvalidMadtLength);
    }

    topology.madt_address = address;
    topology.local_apic_address = read_u32(madt, 36)? as u64;
    topology.madt_flags = read_u32(madt, 40)?;

    let mut offset = MADT_FIXED_LENGTH;
    let mut lapic_override_seen = false;

    while offset < madt.len() {
        if offset + 2 > madt.len() {
            return Err(AcpiError::InvalidMadtEntry);
        }

        let entry_type = madt[offset];
        let entry_length = madt[offset + 1] as usize;
        if entry_length < 2 || offset + entry_length > madt.len() {
            return Err(AcpiError::InvalidMadtEntry);
        }

        let entry = &madt[offset..offset + entry_length];

        match entry_type {
            0 => {
                require_len(entry, 8)?;
                topology.push_cpu(CpuDescriptor {
                    acpi_uid: entry[2] as u32,
                    apic_id: entry[3] as u32,
                    flags: read_u32(entry, 4)?,
                    x2apic: false,
                })?;
            }
            1 => {
                require_len(entry, 12)?;
                topology.push_io_apic(IoApicDescriptor {
                    id: entry[2],
                    address: read_u32(entry, 4)? as u64,
                    gsi_base: read_u32(entry, 8)?,
                })?;
            }
            2 => {
                require_len(entry, 10)?;
                topology.push_iso(InterruptSourceOverride {
                    bus: entry[2],
                    source: entry[3],
                    gsi: read_u32(entry, 4)?,
                    flags: read_u16(entry, 8)?,
                })?;
            }
            3 => {
                require_len(entry, 8)?;
                topology.push_nmi(NmiDescriptor {
                    entry_type,
                    acpi_uid: u32::MAX,
                    gsi: read_u32(entry, 4)?,
                    flags: read_u16(entry, 2)?,
                    lint: 0,
                })?;
            }
            4 => {
                require_len(entry, 6)?;
                topology.push_nmi(NmiDescriptor {
                    entry_type,
                    acpi_uid: entry[2] as u32,
                    gsi: 0,
                    flags: read_u16(entry, 3)?,
                    lint: entry[5],
                })?;
            }
            5 => {
                require_len(entry, 12)?;
                if lapic_override_seen {
                    return Err(AcpiError::InvalidMadtEntry);
                }
                topology.local_apic_address = read_u64(entry, 4)?;
                lapic_override_seen = true;
            }
            9 => {
                require_len(entry, 16)?;
                topology.push_cpu(CpuDescriptor {
                    acpi_uid: read_u32(entry, 12)?,
                    apic_id: read_u32(entry, 4)?,
                    flags: read_u32(entry, 8)?,
                    x2apic: true,
                })?;
            }
            10 => {
                require_len(entry, 12)?;
                topology.push_nmi(NmiDescriptor {
                    entry_type,
                    acpi_uid: read_u32(entry, 4)?,
                    gsi: 0,
                    flags: read_u16(entry, 2)?,
                    lint: entry[8],
                })?;
            }
            _ => {}
        }

        offset += entry_length;
    }

    Ok(())
}

fn require_len(entry: &[u8], minimum: usize) -> Result<(), AcpiError> {
    if entry.len() < minimum {
        Err(AcpiError::InvalidMadtEntry)
    } else {
        Ok(())
    }
}

fn signature(table: &[u8]) -> &[u8] {
    &table[..4]
}

fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, AcpiError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or(AcpiError::InvalidMadtEntry)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, AcpiError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(AcpiError::InvalidMadtEntry)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, AcpiError> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(AcpiError::InvalidMadtEntry)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}
