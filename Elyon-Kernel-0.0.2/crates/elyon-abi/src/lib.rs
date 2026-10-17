#![no_std]

pub const PAGE_SIZE: u64 = 4096;
pub const MAX_MEMORY_REGIONS: usize = 256;
pub const BOOT_INFO_MAGIC: u64 = 0x454C_594F_4E42_4932; // "ELYONBI2"
pub const BOOT_INFO_VERSION: u32 = 2;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryRegionKind {
    Empty = 0,
    Usable = 1,
    Reclaimable = 2,
    Bootstrap = 3,
    FirmwareReserved = 4,
    Mmio = 5,
    Unusable = 6,
    Reserved = 7,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MemoryRegion {
    pub start: u64,
    pub pages: u64,
    pub uefi_type: u32,
    pub kind: MemoryRegionKind,
    pub _padding: [u8; 3],
}

impl MemoryRegion {
    pub const EMPTY: Self = Self {
        start: 0,
        pages: 0,
        uefi_type: 0,
        kind: MemoryRegionKind::Empty,
        _padding: [0; 3],
    };

    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.pages.saturating_mul(PAGE_SIZE))
    }
}

#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub boot_services_exited: u8,
    pub memory_map_valid: u8,
    pub memory_map_truncated: u8,
    pub _flags_padding: u8,
    pub memory_region_count: u32,
    pub _count_padding: u32,
    pub usable_memory_bytes: u64,
    pub highest_physical_address: u64,
    pub kernel_phys_start: u64,
    pub kernel_phys_end: u64,
    pub kernel_stack_start: u64,
    pub kernel_stack_end: u64,
    pub boot_info_phys_start: u64,
    pub boot_info_phys_end: u64,
    pub acpi_rsdp: u64,
    pub memory_regions: [MemoryRegion; MAX_MEMORY_REGIONS],
}

impl BootInfo {
    pub const fn empty() -> Self {
        Self {
            magic: BOOT_INFO_MAGIC,
            version: BOOT_INFO_VERSION,
            boot_services_exited: 0,
            memory_map_valid: 0,
            memory_map_truncated: 0,
            _flags_padding: 0,
            memory_region_count: 0,
            _count_padding: 0,
            usable_memory_bytes: 0,
            highest_physical_address: 0,
            kernel_phys_start: 0,
            kernel_phys_end: 0,
            kernel_stack_start: 0,
            kernel_stack_end: 0,
            boot_info_phys_start: 0,
            boot_info_phys_end: 0,
            acpi_rsdp: 0,
            memory_regions: [MemoryRegion::EMPTY; MAX_MEMORY_REGIONS],
        }
    }

    pub fn sort_and_validate_memory_map(&mut self) {
        let count = self.memory_region_count as usize;

        for i in 1..count {
            let mut j = i;
            while j > 0 && self.memory_regions[j - 1].start > self.memory_regions[j].start {
                self.memory_regions.swap(j - 1, j);
                j -= 1;
            }
        }

        let mut valid = !self.memory_map_truncated.eq(&1);
        let mut previous_end = 0u64;

        for (index, region) in self.memory_regions[..count].iter().enumerate() {
            if region.pages == 0 || region.start % PAGE_SIZE != 0 {
                valid = false;
                break;
            }

            let Some(end) = region.start.checked_add(region.pages.saturating_mul(PAGE_SIZE)) else {
                valid = false;
                break;
            };

            if index > 0 && region.start < previous_end {
                valid = false;
                break;
            }

            previous_end = end;
        }

        self.memory_map_valid = u8::from(valid);
    }
}
