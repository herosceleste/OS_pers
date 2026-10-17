pub const MAX_MEMORY_REGIONS: usize = 192;

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
}

impl MemoryRegion {
    pub const EMPTY: Self = Self {
        start: 0,
        pages: 0,
        uefi_type: 0,
        kind: MemoryRegionKind::Empty,
    };
}

#[repr(C)]
pub struct BootInfo {
    pub boot_services_exited: bool,
    pub memory_map_truncated: bool,
    pub memory_region_count: usize,
    pub total_memory_bytes: u64,
    pub usable_memory_bytes: u64,
    pub memory_regions: [MemoryRegion; MAX_MEMORY_REGIONS],
}

impl BootInfo {
    pub const fn empty() -> Self {
        Self {
            boot_services_exited: false,
            memory_map_truncated: false,
            memory_region_count: 0,
            total_memory_bytes: 0,
            usable_memory_bytes: 0,
            memory_regions: [MemoryRegion::EMPTY; MAX_MEMORY_REGIONS],
        }
    }
}
