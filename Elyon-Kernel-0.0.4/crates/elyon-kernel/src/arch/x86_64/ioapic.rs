use core::ptr::{read_volatile, write_volatile};

use elyon_abi::BootInfo;
use elyon_core::ioapic::{self as plan, LEGACY_IRQ_COUNT, REDIR_MASKED};
pub use elyon_core::ioapic::{IoApicError, IoApicState, LegacyRoute};

use crate::acpi::{AcpiTopology, MAX_IO_APICS};
use crate::arch::x86_64::paging;

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;

const IOAPIC_REG_ID: u8 = 0x00;
const IOAPIC_REG_VERSION: u8 = 0x01;
const IOAPIC_REG_ARBITRATION: u8 = 0x02;

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
}

/// Étape 1, avant le démarrage des AP : valide et mappe chaque IOAPIC, puis
/// masque toutes ses entrées. Aucune IRQ externe ne peut ensuite atteindre un
/// CPU dont IF=1, et le passage UC des pages MMIO a lieu tant que seul le BSP
/// tourne (pas de TLB shootdown nécessaire).
pub fn discover_and_mask(
    topology: &AcpiTopology,
    info: &BootInfo,
) -> Result<IoApicSummary, IoApicError> {
    if topology.io_apics().is_empty() {
        return Err(IoApicError::NoController);
    }

    let regions = &info.memory_regions[..info.memory_region_count as usize];
    let protected = [
        (info.kernel_phys_start, info.kernel_phys_end),
        (info.kernel_stack_start, info.kernel_stack_end),
        (info.boot_info_phys_start, info.boot_info_phys_end),
    ];

    let mut summary = IoApicSummary::empty();

    for descriptor in topology.io_apics() {
        plan::validate_mmio_window(
            descriptor.address,
            paging::identity_limit(),
            regions,
            &protected,
        )?;
        if !paging::mark_identity_mmio_uncacheable(descriptor.address) {
            return Err(IoApicError::MmioOutsideIdentityMap);
        }

        let state = plan::decode_controller(
            descriptor,
            read_reg(descriptor.address, IOAPIC_REG_ID),
            read_reg(descriptor.address, IOAPIC_REG_VERSION),
            read_reg(descriptor.address, IOAPIC_REG_ARBITRATION),
        )?;
        plan::check_new_controller(summary.controllers(), &state)?;

        if !state.id_matches() {
            summary.id_mismatches += 1;
        }

        mask_controller(state);
        summary.total_redirection_entries += state.redirection_entries;
        summary.push_controller(state)?;
    }

    summary.masked_entries = count_masked_entries(summary.controllers());
    if !summary.all_masked() {
        return Err(IoApicError::ReadBackMismatch);
    }

    Ok(summary)
}

/// Étape 2 : programme, toujours masquées, les routes legacy vers le BSP.
pub fn route_legacy(
    summary: &mut IoApicSummary,
    topology: &AcpiTopology,
    bsp_apic_id: u32,
) -> Result<(), IoApicError> {
    let destination =
        u8::try_from(bsp_apic_id).map_err(|_| IoApicError::DestinationApicIdTooLarge)?;

    let legacy = plan::plan_legacy_routes(topology.isos(), summary.controllers())?;
    summary.overrides_applied = legacy.overrides_applied;
    summary.shadowed_legacy_irqs = legacy.shadowed_legacy_irqs;

    for route in legacy.routes() {
        program_route(summary.controllers()[route.controller], route, destination)?;
        summary.legacy_routes[summary.legacy_route_count] = *route;
        summary.legacy_route_count += 1;
    }

    summary.masked_entries = count_masked_entries(summary.controllers());
    if !summary.all_masked() {
        return Err(IoApicError::ReadBackMismatch);
    }

    Ok(())
}

fn count_masked_entries(controllers: &[IoApicState]) -> u32 {
    let mut masked = 0u32;

    for controller in controllers {
        for index in 0..controller.redirection_entries {
            let (low_reg, _) = plan::redirection_registers(index);
            if read_reg(controller.address, low_reg) & REDIR_MASKED != 0 {
                masked += 1;
            }
        }
    }

    masked
}

fn mask_controller(controller: IoApicState) {
    for index in 0..controller.redirection_entries {
        let (low_reg, _) = plan::redirection_registers(index);
        let low = read_reg(controller.address, low_reg) | REDIR_MASKED;
        write_reg(controller.address, low_reg, low);
    }
}

fn program_route(
    controller: IoApicState,
    route: &LegacyRoute,
    destination_apic_id: u8,
) -> Result<(), IoApicError> {
    let (low_reg, high_reg) = plan::redirection_registers(route.redirection_index);
    let (low, high) = plan::encode_route(route, destination_apic_id);

    write_reg(controller.address, high_reg, high);
    write_reg(controller.address, low_reg, low);

    let read = (
        read_reg(controller.address, low_reg),
        read_reg(controller.address, high_reg),
    );
    if !plan::read_back_matches((low, high), read) {
        return Err(IoApicError::ReadBackMismatch);
    }

    Ok(())
}

fn read_reg(base: u64, register: u8) -> u32 {
    // SAFETY: `base` a passé `validate_mmio_window` (non nul, aligné, hors RAM et
    // hors mémoire noyau, sous la limite identité) puis a été mappé UC par
    // `paging`. IOREGSEL/IOWIN sont des registres 32 bits alignés. Les accès ont
    // lieu pendant l'init BSP, seul utilisateur de l'IOAPIC à ce stade.
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, register as u32);
        read_volatile((base + IOWIN) as *const u32)
    }
}

fn write_reg(base: u64, register: u8, value: u32) {
    // SAFETY: mêmes invariants que `read_reg`.
    unsafe {
        write_volatile((base + IOREGSEL) as *mut u32, register as u32);
        write_volatile((base + IOWIN) as *mut u32, value);
    }
}
