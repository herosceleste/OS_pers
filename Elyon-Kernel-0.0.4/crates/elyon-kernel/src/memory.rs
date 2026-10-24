use core::ptr::{addr_of_mut, write_bytes};

use elyon_abi::{BootInfo, MemoryRegionKind, PAGE_SIZE};

use elyon_core::lock_order::LockClass;

use crate::arch::x86_64::paging;
use crate::sync::IrqSpinLock;

const MAX_PHYSICAL_MEMORY: u64 = 32 * 1024 * 1024 * 1024;
const MAX_FRAMES: usize = (MAX_PHYSICAL_MEMORY / PAGE_SIZE) as usize;
const BITMAP_BYTES: usize = MAX_FRAMES.div_ceil(8);
const LOW_MEMORY_RESERVE_END: u64 = 1024 * 1024;

// Zero-initialised so the bitmap lives in .bss instead of inflating KERNEL.ELF
// by 1 MiB. `init()` marks every frame as used before freeing usable regions.
static mut FRAME_BITMAP: [u8; BITMAP_BYTES] = [0; BITMAP_BYTES];
static mut FREE_FRAMES: usize = 0;
static mut MANAGED_FRAMES: usize = 0;
/// Sérialise tout accès au bitmap et aux compteurs ci-dessus (K004-10) : les AP
/// peuvent allouer et libérer. Le bitmap reste un `static mut` pour rester en
/// `.bss`, et n'est touché que verrou tenu.
static PMM_LOCK: IrqSpinLock<()> = IrqSpinLock::new(LockClass::Pmm, ());

pub struct PmmStats {
    pub managed_frames: usize,
    pub free_frames: usize,
    pub managed_mib: u64,
}

pub fn init(info: &BootInfo) -> PmmStats {
    let _pmm = PMM_LOCK.lock();
    unsafe {
        write_bytes(addr_of_mut!(FRAME_BITMAP).cast::<u8>(), 0xFF, BITMAP_BYTES);
        FREE_FRAMES = 0;
    }

    // Manage only RAM the firmware reports as usable, and only what the
    // initial Elyon identity map can reach. Reserved or MMIO ranges high in the
    // address space must not inflate the managed span, and a frame above the
    // identity limit would fault as soon as the kernel touched it.
    let highest_usable = info.memory_regions[..info.memory_region_count as usize]
        .iter()
        .filter(|region| region.kind == MemoryRegionKind::Usable)
        .map(|region| region.end())
        .max()
        .unwrap_or(0);
    let managed_limit = highest_usable
        .min(paging::identity_limit())
        .min(MAX_PHYSICAL_MEMORY);
    let managed_frames = (managed_limit / PAGE_SIZE) as usize;
    unsafe {
        MANAGED_FRAMES = managed_frames;
    }

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

    // Never hand out the historical x86 low-memory area by default.
    // It may contain firmware data, trampoline space or compatibility structures.
    reserve_range(0, LOW_MEMORY_RESERVE_END);
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
    allocate_contiguous_frames(1)
}

pub fn allocate_contiguous_frames(count: usize) -> Option<u64> {
    let _pmm = PMM_LOCK.lock();
    let managed_frames = unsafe { MANAGED_FRAMES };
    if count == 0 || count > managed_frames {
        return None;
    }

    let first_allocatable = (LOW_MEMORY_RESERVE_END / PAGE_SIZE) as usize;
    let last_start = managed_frames.checked_sub(count)?;

    'candidate: for start in first_allocatable..=last_start {
        for frame in start..start + count {
            if !is_free(frame) {
                continue 'candidate;
            }
        }

        for frame in start..start + count {
            mark_used(frame);
        }
        return Some(start as u64 * PAGE_SIZE);
    }

    None
}

/// Rend `count` pages contiguës commençant à `start`. Refuse, sans rien
/// modifier, une plage hors du domaine géré ou contenant une page déjà libre :
/// une double libération serait sinon rendue deux fois.
pub fn free_contiguous_frames(start: u64, count: usize) -> bool {
    let _pmm = PMM_LOCK.lock();
    let managed_frames = unsafe { MANAGED_FRAMES };
    if count == 0 || !start.is_multiple_of(PAGE_SIZE) {
        return false;
    }
    let first = (start / PAGE_SIZE) as usize;
    let first_allocatable = (LOW_MEMORY_RESERVE_END / PAGE_SIZE) as usize;
    if first < first_allocatable || first.saturating_add(count) > managed_frames {
        return false;
    }
    if (first..first + count).any(is_free) {
        return false;
    }
    for frame in first..first + count {
        mark_free(frame);
    }
    true
}

pub fn free_frame(start: u64) -> bool {
    free_contiguous_frames(start, 1)
}

pub fn free_frame_count() -> usize {
    let _pmm = PMM_LOCK.lock();
    unsafe { FREE_FRAMES }
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

pub fn range_is_usable(info: &BootInfo, start: u64, end: u64) -> bool {
    if start == 0 || end <= start {
        return false;
    }

    info.memory_regions[..info.memory_region_count as usize]
        .iter()
        .any(|region| {
            region.kind == MemoryRegionKind::Usable && start >= region.start && end <= region.end()
        })
}
