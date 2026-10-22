//! Logique IOAPIC sans MMIO : décodage des registres, cohérence des
//! contrôleurs, plan de routage des 16 IRQ legacy et encodage des entrées de
//! redirection. Le noyau lit et écrit les registres ; ce module décide.

use elyon_abi::{MemoryRegion, MemoryRegionKind, PAGE_SIZE};

use crate::acpi::{InterruptSourceOverride, IoApicDescriptor};

pub const IOAPIC_REDIR_BASE: u8 = 0x10;

pub const REDIR_DELIVERY_STATUS: u32 = 1 << 12;
pub const REDIR_POLARITY_LOW: u32 = 1 << 13;
pub const REDIR_REMOTE_IRR: u32 = 1 << 14;
pub const REDIR_TRIGGER_LEVEL: u32 = 1 << 15;
pub const REDIR_MASKED: u32 = 1 << 16;

/// Bits du mot bas relus pour valider une entrée : vecteur, mode de livraison,
/// mode de destination, polarité, déclenchement et masque. Delivery Status et
/// Remote IRR sont en lecture seule et varient avec l'état de la ligne.
const REDIR_LOW_CHECKED: u32 = 0x0001_AFFF;
const REDIR_HIGH_DESTINATION: u32 = 0xFF00_0000;

pub const LEGACY_IRQ_COUNT: usize = 16;
pub const LEGACY_VECTOR_BASE: u8 = 0x20;
const MAX_IOAPIC_REDIRECTIONS: u32 = 120;

/// IOREGSEL et IOWIN sont des registres 32 bits à 0x00 et 0x10.
const MMIO_ALIGNMENT: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApicState {
    pub madt_id: u8,
    pub hw_id: u8,
    pub version: u8,
    pub arbitration_id: u8,
    pub address: u64,
    pub gsi_base: u32,
    pub redirection_entries: u32,
}

impl IoApicState {
    pub const EMPTY: Self = Self {
        madt_id: 0,
        hw_id: 0,
        version: 0,
        arbitration_id: 0,
        address: 0,
        gsi_base: 0,
        redirection_entries: 0,
    };

    pub const fn gsi_end_exclusive(self) -> u32 {
        self.gsi_base + self.redirection_entries
    }

    pub const fn id_matches(self) -> bool {
        self.madt_id == self.hw_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyRoute {
    pub irq: u8,
    pub gsi: u32,
    pub vector: u8,
    pub active_low: bool,
    pub level_triggered: bool,
    pub overridden: bool,
    pub masked: bool,
    /// Index du contrôleur dans la tranche passée à [`plan_legacy_routes`].
    pub controller: usize,
    pub redirection_index: u32,
}

impl LegacyRoute {
    pub const EMPTY: Self = Self {
        irq: 0,
        gsi: 0,
        vector: 0,
        active_low: false,
        level_triggered: false,
        overridden: false,
        masked: true,
        controller: 0,
        redirection_index: 0,
    };

    pub const fn polarity_str(self) -> &'static str {
        if self.active_low { "low" } else { "high" }
    }

    pub const fn trigger_str(self) -> &'static str {
        if self.level_triggered {
            "level"
        } else {
            "edge"
        }
    }
}

pub struct LegacyPlan {
    routes: [LegacyRoute; LEGACY_IRQ_COUNT],
    route_count: usize,
    pub overrides_applied: usize,
    pub shadowed_legacy_irqs: usize,
}

impl LegacyPlan {
    pub fn routes(&self) -> &[LegacyRoute] {
        &self.routes[..self.route_count]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoApicError {
    NoController,
    InvalidAddress,
    MmioOutsideIdentityMap,
    MmioMisaligned,
    MmioOverlapsRam,
    MmioOverlapsKernel,
    ControllerCapacityExceeded,
    RedirectionCountInvalid,
    GsiRangeOverflow,
    GsiRangeOverlap,
    DuplicateMadtId,
    DuplicateControllerAddress,
    DestinationApicIdTooLarge,
    UnsupportedOverrideBus,
    InvalidOverrideFlags,
    DuplicateOverride,
    DuplicateRoute,
    RouteOutsideIoApic,
    ReadBackMismatch,
}

impl IoApicError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoController => "no IOAPIC discovered by MADT",
            Self::InvalidAddress => "IOAPIC MMIO address is zero",
            Self::MmioOutsideIdentityMap => "IOAPIC MMIO outside current identity map",
            Self::MmioMisaligned => "IOAPIC MMIO address is not 16-byte aligned",
            Self::MmioOverlapsRam => "IOAPIC MMIO overlaps RAM from the memory map",
            Self::MmioOverlapsKernel => "IOAPIC MMIO overlaps kernel-owned memory",
            Self::ControllerCapacityExceeded => "IOAPIC controller capacity exceeded",
            Self::RedirectionCountInvalid => "invalid IOAPIC redirection entry count",
            Self::GsiRangeOverflow => "IOAPIC GSI range overflow",
            Self::GsiRangeOverlap => "overlapping IOAPIC GSI ranges",
            Self::DuplicateMadtId => "duplicate MADT IOAPIC ID",
            Self::DuplicateControllerAddress => "duplicate IOAPIC MMIO address",
            Self::DestinationApicIdTooLarge => {
                "BSP APIC ID does not fit IOAPIC physical destination"
            }
            Self::UnsupportedOverrideBus => "unsupported interrupt source override bus",
            Self::InvalidOverrideFlags => "reserved ACPI interrupt override flags",
            Self::DuplicateOverride => "duplicate interrupt source override",
            Self::DuplicateRoute => "multiple legacy IRQs map to the same GSI",
            Self::RouteOutsideIoApic => "legacy GSI not covered by an IOAPIC",
            Self::ReadBackMismatch => "IOAPIC redirection read-back mismatch",
        }
    }
}

/// Vérifie qu'une adresse MMIO annoncée par la MADT peut être passée en UC et
/// écrite sans toucher la RAM ni la mémoire du noyau. `protected` liste des
/// plages `[start, end)` possédées par Elyon (image, pile, BootInfo).
pub fn validate_mmio_window(
    address: u64,
    identity_limit: u64,
    regions: &[MemoryRegion],
    protected: &[(u64, u64)],
) -> Result<(), IoApicError> {
    if address == 0 {
        return Err(IoApicError::InvalidAddress);
    }
    if address % MMIO_ALIGNMENT != 0 {
        return Err(IoApicError::MmioMisaligned);
    }

    let page_start = address - address % PAGE_SIZE;
    let page_end = page_start
        .checked_add(PAGE_SIZE)
        .ok_or(IoApicError::MmioOutsideIdentityMap)?;
    if page_end > identity_limit {
        return Err(IoApicError::MmioOutsideIdentityMap);
    }

    let overlaps_ram = regions.iter().any(|region| {
        is_ram(region.kind) && ranges_overlap(page_start, page_end, region.start, region.end())
    });
    if overlaps_ram {
        return Err(IoApicError::MmioOverlapsRam);
    }

    if protected
        .iter()
        .any(|&(start, end)| ranges_overlap(page_start, page_end, start, end))
    {
        return Err(IoApicError::MmioOverlapsKernel);
    }

    Ok(())
}

/// Construit l'état d'un contrôleur à partir de sa description MADT et de ses
/// registres ID (0x00), VERSION (0x01) et ARBITRATION (0x02).
pub fn decode_controller(
    descriptor: &IoApicDescriptor,
    id_raw: u32,
    version_raw: u32,
    arbitration_raw: u32,
) -> Result<IoApicState, IoApicError> {
    let redirection_entries = ((version_raw >> 16) & 0xFF) + 1;
    if redirection_entries > MAX_IOAPIC_REDIRECTIONS {
        return Err(IoApicError::RedirectionCountInvalid);
    }

    descriptor
        .gsi_base
        .checked_add(redirection_entries)
        .ok_or(IoApicError::GsiRangeOverflow)?;

    Ok(IoApicState {
        madt_id: descriptor.id,
        hw_id: ((id_raw >> 24) & 0x0F) as u8,
        version: (version_raw & 0xFF) as u8,
        arbitration_id: ((arbitration_raw >> 24) & 0x0F) as u8,
        address: descriptor.address,
        gsi_base: descriptor.gsi_base,
        redirection_entries,
    })
}

/// Rejette un contrôleur qui duplique l'ID MADT, l'adresse ou une partie de la
/// plage GSI d'un contrôleur déjà accepté. Une divergence entre ID MADT et ID
/// matériel n'est pas une erreur : elle reste un diagnostic.
pub fn check_new_controller(
    existing: &[IoApicState],
    state: &IoApicState,
) -> Result<(), IoApicError> {
    for known in existing {
        if known.madt_id == state.madt_id {
            return Err(IoApicError::DuplicateMadtId);
        }
        if known.address == state.address {
            return Err(IoApicError::DuplicateControllerAddress);
        }
        if ranges_overlap(
            state.gsi_base as u64,
            state.gsi_end_exclusive() as u64,
            known.gsi_base as u64,
            known.gsi_end_exclusive() as u64,
        ) {
            return Err(IoApicError::GsiRangeOverlap);
        }
    }
    Ok(())
}

/// Calcule les routes des IRQ ISA 0..15 en appliquant les Interrupt Source
/// Overrides. Une IRQ sans override dont le GSI est capturé par l'override
/// d'une autre IRQ est masquée (« shadowed ») et n'obtient pas de route.
pub fn plan_legacy_routes(
    overrides: &[InterruptSourceOverride],
    controllers: &[IoApicState],
) -> Result<LegacyPlan, IoApicError> {
    if controllers.is_empty() {
        return Err(IoApicError::NoController);
    }

    let mut plan = LegacyPlan {
        routes: [LegacyRoute::EMPTY; LEGACY_IRQ_COUNT],
        route_count: 0,
        overrides_applied: 0,
        shadowed_legacy_irqs: 0,
    };

    for irq in 0..LEGACY_IRQ_COUNT as u8 {
        let mut gsi = irq as u32;
        let mut active_low = false;
        let mut level_triggered = false;
        let mut overridden = false;

        if let Some(iso) = find_override(overrides, irq)? {
            if iso.bus != 0 {
                return Err(IoApicError::UnsupportedOverrideBus);
            }

            let (polarity_low, trigger_level) = decode_iso_flags(*iso)?;
            gsi = iso.gsi;
            active_low = polarity_low;
            level_triggered = trigger_level;
            overridden = true;
            plan.overrides_applied += 1;
        }

        if !overridden && gsi_reserved_by_other_override(overrides, irq, gsi) {
            plan.shadowed_legacy_irqs += 1;
            continue;
        }

        if plan.routes().iter().any(|route| route.gsi == gsi) {
            return Err(IoApicError::DuplicateRoute);
        }

        let (controller, redirection_index) =
            find_controller_for_gsi(controllers, gsi).ok_or(IoApicError::RouteOutsideIoApic)?;

        plan.routes[plan.route_count] = LegacyRoute {
            irq,
            gsi,
            vector: LEGACY_VECTOR_BASE + irq,
            active_low,
            level_triggered,
            overridden,
            masked: true,
            controller,
            redirection_index,
        };
        plan.route_count += 1;
    }

    Ok(plan)
}

/// Registres (bas, haut) de l'entrée de redirection `index`.
pub const fn redirection_registers(index: u32) -> (u8, u8) {
    let low = IOAPIC_REDIR_BASE + (index as u8) * 2;
    (low, low + 1)
}

/// Encode une route en mots (bas, haut) : livraison fixe, destination physique.
pub fn encode_route(route: &LegacyRoute, destination_apic_id: u8) -> (u32, u32) {
    let mut low = route.vector as u32;
    if route.masked {
        low |= REDIR_MASKED;
    }
    if route.active_low {
        low |= REDIR_POLARITY_LOW;
    }
    if route.level_triggered {
        low |= REDIR_TRIGGER_LEVEL;
    }

    (low, (destination_apic_id as u32) << 24)
}

/// Compare une entrée relue à l'entrée écrite, en ignorant les bits d'état en
/// lecture seule et les bits réservés du mot haut.
pub const fn read_back_matches(expected: (u32, u32), read: (u32, u32)) -> bool {
    read.0 & REDIR_LOW_CHECKED == expected.0 & REDIR_LOW_CHECKED
        && read.1 & REDIR_HIGH_DESTINATION == expected.1 & REDIR_HIGH_DESTINATION
}

fn find_override(
    overrides: &[InterruptSourceOverride],
    irq: u8,
) -> Result<Option<&InterruptSourceOverride>, IoApicError> {
    let mut found = None;

    for iso in overrides {
        if iso.source != irq {
            continue;
        }
        if found.is_some() {
            return Err(IoApicError::DuplicateOverride);
        }
        found = Some(iso);
    }

    Ok(found)
}

fn gsi_reserved_by_other_override(
    overrides: &[InterruptSourceOverride],
    irq: u8,
    gsi: u32,
) -> bool {
    overrides
        .iter()
        .any(|iso| iso.bus == 0 && iso.source != irq && iso.gsi == gsi)
}

fn decode_iso_flags(iso: InterruptSourceOverride) -> Result<(bool, bool), IoApicError> {
    let polarity = iso.flags & 0b11;
    let trigger = (iso.flags >> 2) & 0b11;

    let active_low = match polarity {
        0 | 1 => false,
        3 => true,
        _ => return Err(IoApicError::InvalidOverrideFlags),
    };

    let level_triggered = match trigger {
        0 | 1 => false,
        3 => true,
        _ => return Err(IoApicError::InvalidOverrideFlags),
    };

    Ok((active_low, level_triggered))
}

fn find_controller_for_gsi(controllers: &[IoApicState], gsi: u32) -> Option<(usize, u32)> {
    controllers
        .iter()
        .position(|controller| gsi >= controller.gsi_base && gsi < controller.gsi_end_exclusive())
        .map(|index| (index, gsi - controllers[index].gsi_base))
}

const fn is_ram(kind: MemoryRegionKind) -> bool {
    matches!(
        kind,
        MemoryRegionKind::Usable | MemoryRegionKind::Reclaimable | MemoryRegionKind::Bootstrap
    )
}

const fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}
