#![no_main]
#![no_std]

use core::panic::PanicInfo;

use elyon_kernel::bootinfo::{BootInfo, MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionKind};
use elyon_kernel::{kernel_main, kprintln};
use uefi::boot::MemoryType;
use uefi::mem::memory_map::MemoryMap;
use uefi::{Status, entry};

const PAGE_SIZE: u64 = 4096;

#[entry]
fn main() -> Status {
    // Initialises uefi-rs internal helpers while Boot Services are still available.
    // The kernel itself does not depend on these helpers after ExitBootServices.
    uefi::helpers::init().expect("UEFI helper initialisation failed");

    elyon_kernel::serial::init();
    kprintln!("ELYON BOOT 0.0.1");
    kprintln!("[boot] architecture=x86_64 firmware=UEFI");
    kprintln!("[boot] preparing final UEFI memory map");

    // Safety: this is the single, deliberate transition out of UEFI Boot Services.
    // No Boot Services call is performed after this point.
    let memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    let mut info = BootInfo::empty();
    let mut index = 0usize;

    for descriptor in memory_map.entries() {
        let bytes = descriptor.page_count.saturating_mul(PAGE_SIZE);
        info.total_memory_bytes = info.total_memory_bytes.saturating_add(bytes);

        let kind = classify_memory(descriptor.ty);
        if matches!(kind, MemoryRegionKind::Usable) {
            info.usable_memory_bytes = info.usable_memory_bytes.saturating_add(bytes);
        }

        if index < MAX_MEMORY_REGIONS {
            info.memory_regions[index] = MemoryRegion {
                start: descriptor.phys_start,
                pages: descriptor.page_count,
                uefi_type: descriptor.ty.0,
                kind,
            };
            index += 1;
        } else {
            info.memory_map_truncated = true;
        }
    }

    info.memory_region_count = index;
    info.boot_services_exited = true;

    kernel_main(&info)
}

fn classify_memory(ty: MemoryType) -> MemoryRegionKind {
    if ty == MemoryType::CONVENTIONAL {
        MemoryRegionKind::Usable
    } else if ty == MemoryType::BOOT_SERVICES_CODE
        || ty == MemoryType::BOOT_SERVICES_DATA
        || ty == MemoryType::ACPI_RECLAIM
    {
        MemoryRegionKind::Reclaimable
    } else if ty == MemoryType::LOADER_CODE || ty == MemoryType::LOADER_DATA {
        // Elyon 0.0.1 is still a single UEFI image. Its executable image and
        // stack may reside in loader memory, so this memory is kept reserved.
        MemoryRegionKind::Bootstrap
    } else if ty == MemoryType::RUNTIME_SERVICES_CODE
        || ty == MemoryType::RUNTIME_SERVICES_DATA
        || ty == MemoryType::ACPI_NON_VOLATILE
    {
        MemoryRegionKind::FirmwareReserved
    } else if ty == MemoryType::MMIO || ty == MemoryType::MMIO_PORT_SPACE {
        MemoryRegionKind::Mmio
    } else if ty == MemoryType::UNUSABLE {
        MemoryRegionKind::Unusable
    } else {
        MemoryRegionKind::Reserved
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    elyon_kernel::panic_report(info)
}
