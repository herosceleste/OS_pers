use core::ptr::{read_volatile, write_volatile};

use crate::acpi::{AcpiTopology, InterruptSourceOverride, MAX_IO_APICS};
use crate::arch::x86_64::paging;

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;

const IOAPIC_REG_ID: u8 = 0x00;
const IOAPIC_REG_VERSION: u8 = 0x01;
const IOAPIC_REG_ARBITRATION: u8 = 0x02;
const IOAPIC_REDIR_BASE: u8 = 0x10;

const REDIR_POLARITY_LOW: u32 = 1 << 13;
const REDIR_TRIGGER_LEVEL: u32 = 1 << 15;
const REDIR_MASKED: u32 = 1 << 16;

const LEGACY_IRQ_COUNT: usize = 16;
const MAX_IOAPIC_REDIRECTIONS: u32 = 120;
pub const LEGACY_VECTOR_BASE: u8 = 0x20;

#[derive(Clone, Copy)]
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
    const EMPTY: Self = Self {
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

#[derive(Clone, Copy)]
pub struct LegacyRoute {
    pub irq: u8,
    pub gsi: u32,
    pub vector: u8,
    pub active_low: bool,
    pub level_triggered: bool,
    pub overridden: bool,
    pub masked: bool,
}

impl LegacyRoute {
    const EMPTY: Self = Self {
        irq: 0,
        gsi: 0,
        vector: 0,
        active_low: false,
        level_triggered: false,
        overridden: false,
        masked: true,
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

pub struct IoApicSummary {
    controllers: [IoApicState; MAX_IO_APICS],
    controller_count: usize,
    legacy_routes: [LegacyRoute; LEGACY_IRQ_COUNT],
    legacy_route_count: usize,
    pub total_redirection_entries: u32,
    pub masked_entries: u32,
    pub overrides_applied: usize,
    pub shadowed_legacy_irqs: usize,
    pub id_mismatches: usize,
}

impl IoApicSummary {
    fn empty() -> Self {
        Self {
            controllers: [IoApicState::EMPTY; MAX_IO_APICS],
            controller_count: 0,
            legacy_routes: [LegacyRoute::EMPTY; LEGACY_IRQ_COUNT],
            legacy_route_count: 0,
            total_redirection_entries: 0,
            masked_entries: 0,
            overrides_applied: 0,
            shadowed_legacy_irqs: 0,
            id_mismatches: 0,
        }
    }

    pub fn controllers(&self) -> &[IoApicState] {
        &self.controllers[..self.controller_count]
    }

    pub fn legacy_routes(&self) -> &[LegacyRoute] {
        &self.legacy_routes[..self.legacy_route_count]
    }

    pub const fn controller_count(&self) -> usize {
        self.controller_count
    }

    pub const fn legacy_route_count(&self) -> usize {
        self.legacy_route_count
    }

    pub const fn all_masked(&self) -> bool {
        self.total_redirection_entries != 0 && self.masked_entries == self.total_redirection_entries
    }

    fn push_controller(&mut self, controller: IoApicState) -> Result<(), IoApicError> {
        if self.controller_count >= MAX_IO_APICS {
            return Err(IoApicError::ControllerCapacityExceeded);
        }
        self.controllers[self.controller_count] = controller;
        self.controller_count += 1;
        Ok(())
    }

    fn push_route(&mut self, route: LegacyRoute) -> Result<(), IoApicError> {
        if self.legacy_route_count >= LEGACY_IRQ_COUNT {
            return Err(IoApicError::RouteCapacityExceeded);
        }
        self.legacy_routes[self.legacy_route_count] = route;
        self.legacy_route_count += 1;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum IoApicError {
    NoController,
    InvalidAddress,
    MmioOutsideIdentityMap,
    ControllerCapacityExceeded,
    RouteCapacityExceeded,
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
            Self::ControllerCapacityExceeded => "IOAPIC controller capacity exceeded",
            Self::RouteCapacityExceeded => "IOAPIC route capacity exceeded",
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

pub fn init(topology: &AcpiTopology, bsp_apic_id: u32) -> Result<IoApicSummary, IoApicError> {
    if topology.io_apics().is_empty() {
        return Err(IoApicError::NoController);
    }
    if bsp_apic_id > u8::MAX as u32 {
        return Err(IoApicError::DestinationApicIdTooLarge);
    }

    let mut summary = IoApicSummary::empty();

    for descriptor in topology.io_apics() {
        if descriptor.address == 0 {
            return Err(IoApicError::InvalidAddress);
        }
        if !paging::mark_identity_mmio_uncacheable(descriptor.address) {
            return Err(IoApicError::MmioOutsideIdentityMap);
        }

        let id_raw = read_reg(descriptor.address, IOAPIC_REG_ID);
        let version_raw = read_reg(descriptor.address, IOAPIC_REG_VERSION);
        let arbitration_raw = read_reg(descriptor.address, IOAPIC_REG_ARBITRATION);

        let redirection_entries = ((version_raw >> 16) & 0xFF) + 1;
        if redirection_entries == 0 || redirection_entries > MAX_IOAPIC_REDIRECTIONS {
            return Err(IoApicError::RedirectionCountInvalid);
        }

        descriptor
            .gsi_base
            .checked_add(redirection_entries)
            .ok_or(IoApicError::GsiRangeOverflow)?;

        let state = IoApicState {
            madt_id: descriptor.id,
            hw_id: ((id_raw >> 24) & 0x0F) as u8,
            version: (version_raw & 0xFF) as u8,
            arbitration_id: ((arbitration_raw >> 24) & 0x0F) as u8,
            address: descriptor.address,
            gsi_base: descriptor.gsi_base,
            redirection_entries,
        };

        if !state.id_matches() {
            summary.id_mismatches += 1;
        }

        for existing in summary.controllers() {
            if existing.madt_id == state.madt_id {
                return Err(IoApicError::DuplicateMadtId);
            }
            if existing.address == state.address {
                return Err(IoApicError::DuplicateControllerAddress);
            }
            if ranges_overlap(
                state.gsi_base,
                state.gsi_end_exclusive(),
                existing.gsi_base,
                existing.gsi_end_exclusive(),
            ) {
                return Err(IoApicError::GsiRangeOverlap);
            }
        }

        mask_controller(state);
        summary.total_redirection_entries += state.redirection_entries;
        summary.push_controller(state)?;
    }

    let mut used_gsis = [u32::MAX; LEGACY_IRQ_COUNT];
    let mut used_count = 0usize;

    for irq in 0..LEGACY_IRQ_COUNT {
        let irq = irq as u8;
        let mut gsi = irq as u32;
        let mut active_low = false;
        let mut level_triggered = false;
        let mut overridden = false;

        if let Some(iso) = find_override(topology.isos(), irq)? {
            if iso.bus != 0 {
                return Err(IoApicError::UnsupportedOverrideBus);
            }

            let (polarity_low, trigger_level) = decode_iso_flags(*iso)?;
            gsi = iso.gsi;
            active_low = polarity_low;
            level_triggered = trigger_level;
            overridden = true;
            summary.overrides_applied += 1;
        }

        if !overridden && gsi_reserved_by_other_override(topology.isos(), irq, gsi)? {
            summary.shadowed_legacy_irqs += 1;
            continue;
        }

        if used_gsis[..used_count].contains(&gsi) {
            return Err(IoApicError::DuplicateRoute);
        }
        used_gsis[used_count] = gsi;
        used_count += 1;

        let (controller, redir_index) = find_controller_for_gsi(summary.controllers(), gsi)
            .ok_or(IoApicError::RouteOutsideIoApic)?;

        let vector = LEGACY_VECTOR_BASE.wrapping_add(irq);
        program_masked_route(
            *controller,
            redir_index,
            vector,
            bsp_apic_id as u8,
            active_low,
            level_triggered,
        )?;

        summary.push_route(LegacyRoute {
            irq,
            gsi,
            vector,
            active_low,
            level_triggered,
            overridden,
            masked: true,
        })?;
    }

    summary.masked_entries = count_masked_entries(summary.controllers());
    if !summary.all_masked() {
        return Err(IoApicError::ReadBackMismatch);
    }

    Ok(summary)
}

fn count_masked_entries(controllers: &[IoApicState]) -> u32 {
    let mut masked = 0u32;

    for controller in controllers {
        for index in 0..controller.redirection_entries {
            let low_reg = IOAPIC_REDIR_BASE.wrapping_add((index * 2) as u8);
            if read_reg(controller.address, low_reg) & REDIR_MASKED != 0 {
                masked += 1;
            }
        }
    }

    masked
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
) -> Result<bool, IoApicError> {
    for iso in overrides {
        if iso.bus != 0 {
            continue;
        }
        if iso.source != irq && iso.gsi == gsi {
            return Ok(true);
        }
    }
    Ok(false)
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

fn find_controller_for_gsi(controllers: &[IoApicState], gsi: u32) -> Option<(&IoApicState, u32)> {
    for controller in controllers {
        if gsi >= controller.gsi_base && gsi < controller.gsi_end_exclusive() {
            return Some((controller, gsi - controller.gsi_base));
        }
    }
    None
}

fn mask_controller(controller: IoApicState) {
    for index in 0..controller.redirection_entries {
        let low_reg = IOAPIC_REDIR_BASE.wrapping_add((index * 2) as u8);
        let low = read_reg(controller.address, low_reg) | REDIR_MASKED;
        write_reg(controller.address, low_reg, low);
    }
}

fn program_masked_route(
    controller: IoApicState,
    redir_index: u32,
    vector: u8,
    destination_apic_id: u8,
    active_low: bool,
    level_triggered: bool,
) -> Result<(), IoApicError> {
    let low_reg = IOAPIC_REDIR_BASE.wrapping_add((redir_index * 2) as u8);
    let high_reg = low_reg.wrapping_add(1);

    let mut low = vector as u32 | REDIR_MASKED;
    if active_low {
        low |= REDIR_POLARITY_LOW;
    }
    if level_triggered {
        low |= REDIR_TRIGGER_LEVEL;
    }

    let high = (destination_apic_id as u32) << 24;

    write_reg(controller.address, high_reg, high);
    write_reg(controller.address, low_reg, low);

    let read_low = read_reg(controller.address, low_reg);
    let read_high = read_reg(controller.address, high_reg);

    let relevant_low = 0x0001_AFFFu32;
    if read_low & relevant_low != low & relevant_low || read_high & 0xFF00_0000 != high {
        return Err(IoApicError::ReadBackMismatch);
    }

    Ok(())
}

fn ranges_overlap(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> bool {
    a_start < b_end && b_start < a_end
}

fn read_reg(base: u64, register: u8) -> u32 {
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, register as u32);
        read_volatile((base + IOWIN) as *const u32)
    }
}

fn write_reg(base: u64, register: u8, value: u32) {
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, register as u32);
        write_volatile((base + IOWIN) as *mut u32, value);
    }
}
