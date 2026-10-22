use core::slice;

use elyon_core::acpi::PhysMemory;
pub use elyon_core::acpi::{
    AcpiError, AcpiTopology, CpuDescriptor, InterruptSourceOverride, MAX_IO_APICS,
};

/// Mémoire physique accessible via la carte identité courante d'Elyon.
struct IdentityMapped {
    mapped_limit: u64,
}

impl PhysMemory for IdentityMapped {
    fn bytes(&self, address: u64, length: usize) -> Result<&[u8], AcpiError> {
        let end = address
            .checked_add(length as u64)
            .ok_or(AcpiError::AddressOutsideMappedRange)?;

        if address == 0 || end > self.mapped_limit {
            return Err(AcpiError::AddressOutsideMappedRange);
        }

        // SAFETY: [address, end) est non nul et entièrement sous `mapped_limit`,
        // donc couvert par la carte identité installée par `paging`, qui reste
        // active pendant toute la vie du noyau. Les tables ACPI ne sont jamais
        // écrites par Elyon : une vue en lecture seule ne crée pas d'alias mutable.
        unsafe { Ok(slice::from_raw_parts(address as *const u8, length)) }
    }
}

pub fn parse(rsdp_address: u64, mapped_limit: u64) -> Result<AcpiTopology, AcpiError> {
    elyon_core::acpi::parse(&IdentityMapped { mapped_limit }, rsdp_address)
}
