use core::ptr::{addr_of_mut, write_bytes};

use elyon_abi::{BootInfo, MemoryRegionKind, PAGE_SIZE};

const MAX_PHYSICAL_MEMORY: u64 = 32 * 1024 * 1024 * 1024;
const MAX_FRAMES: usize = (MAX_PHYSICAL_MEMORY / PAGE_SIZE) as usize;
const BITMAP_BYTES: usize = MAX_FRAMES.div_ceil(8);

static mut FRAME_BITMAP: [u8; BITMAP_BYTES] = [0xFF; BITMAP_BYTES];
static mut FREE_FRAMES: usize = 0;

pub struct PmmStats {
    pub managed_frames: usize,
    pub free_frames: usize,
    pub managed_mib: u64,
}

pub fn init(info: &BootInfo) -> PmmStats {
    unsafe {
        write_bytes(addr_of_mut!(FRAME_BITMAP).cast::<u8>(), 0xFF, BITMAP_BYTES);
        FREE_FRAMES = 0;
    }

    let managed_limit = info.highest_physical_address.min(MAX_PHYSICAL_MEMORY);
    let managed_frames = (managed_limit / PAGE_SIZE) as usize;

    for region in &info.memory_regions[..info.memory_region_count as usize] {
        if region.kind != MemoryRegionKind::Usable {
            continue;
        }
        let start = (region.start / PAGE_SIZE) as usize;
        let end = (region.end().min(managed_limit) / PAGE_SIZE) as usize;
        for frame in start..end {
            mark_free(frame);
        }
    }

    reserve_range(0, PAGE_SIZE);
    reserve_range(info.kernel_phys_start, info.kernel_phys_end);
    reserve_range(info.kernel_stack_start, info.kernel_stack_end);
    reserve_range(info.boot_info_phys_start, info.boot_info_phys_end);

    let free_frames = unsafe { FREE_FRAMES };
    PmmStats {
        managed_frames,
        free_frames,
        managed_mib: managed_limit / 1024 / 1024,
    }
}

pub fn allocate_frame() -> Option<u64> {
    for frame in 1..MAX_FRAMES {
        if is_free(frame) {
            mark_used(frame);
            return Some(frame as u64 * PAGE_SIZE);
        }
    }
    None
}

fn reserve_range(start: u64, end: u64) {
    if end <= start {
        return;
    }
    let first = (start / PAGE_SIZE) as usize;
    let last = end.div_ceil(PAGE_SIZE) as usize;
    for frame in first..last.min(MAX_FRAMES) {
        mark_used(frame);
    }
}

fn is_free(frame: usize) -> bool {
    if frame >= MAX_FRAMES {
        return false;
    }
    let byte = frame / 8;
    let bit = frame % 8;
    unsafe {
        let ptr = addr_of_mut!(FRAME_BITMAP).cast::<u8>().add(byte);
        (*ptr & (1 << bit)) == 0
    }
}

fn mark_free(frame: usize) {
    if frame >= MAX_FRAMES {
        return;
    }
    let byte = frame / 8;
    let bit = frame % 8;
    unsafe {
        let ptr = addr_of_mut!(FRAME_BITMAP).cast::<u8>().add(byte);
        if (*ptr & (1 << bit)) != 0 {
            *ptr &= !(1 << bit);
            FREE_FRAMES = FREE_FRAMES.saturating_add(1);
        }
    }
}

fn mark_used(frame: usize) {
    if frame >= MAX_FRAMES {
        return;
    }
    let byte = frame / 8;
    let bit = frame % 8;
    unsafe {
        let ptr = addr_of_mut!(FRAME_BITMAP).cast::<u8>().add(byte);
        if (*ptr & (1 << bit)) == 0 {
            *ptr |= 1 << bit;
            FREE_FRAMES = FREE_FRAMES.saturating_sub(1);
        }
    }
}
