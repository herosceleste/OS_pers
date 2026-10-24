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
/// Pages allouées par `allocate_owned`, seules rendables par `free` (K004-10).
static mut FREEABLE_BITMAP: [u8; BITMAP_BYTES] = [0; BITMAP_BYTES];
/// Point de départ de la prochaine recherche (next-fit).
static mut NEXT_HINT: usize = 0;
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
        write_bytes(addr_of_mut!(FREEABLE_BITMAP).cast::<u8>(), 0, BITMAP_BYTES);
        FREE_FRAMES = 0;
        NEXT_HINT = 0;
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

/// Allocation permanente, jamais rendue (piles, zones FPU, structures
/// per-CPU) : aucun jeton n'existe pour elle, elle ne peut donc pas être libérée.
/// Les pages sont effacées.
pub fn allocate_contiguous_frames(count: usize) -> Option<u64> {
    let first = {
        let _pmm = PMM_LOCK.lock();
        find_and_mark(count, false)?
    };
    Some(zeroed(first, count))
}

/// Plage de pages rendable, obtenue par `allocate_owned`. Ses champs sont
/// privés : seul le PMM en fabrique, et `free` la consomme, ce qui interdit à la
/// compilation de libérer deux fois ou de libérer une plage arbitraire.
#[must_use = "une plage non rendue reste allouée : la passer à memory::free"]
pub struct FrameRange {
    first: usize,
    count: usize,
}

impl FrameRange {
    pub fn start(&self) -> u64 {
        self.first as u64 * PAGE_SIZE
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PmmError {
    /// Une page de la plage n'est pas marquée libérable : l'état du PMM est
    /// incohérent avec le jeton.
    NotFreeable,
}

/// Allocation rendable par `free`. Les pages sont effacées.
pub fn allocate_owned(count: usize) -> Option<FrameRange> {
    let first = {
        let _pmm = PMM_LOCK.lock();
        find_and_mark(count, true)?
    };
    zeroed(first, count);
    Some(FrameRange { first, count })
}

/// Rend une plage obtenue par `allocate_owned`. Le bitmap des pages libérables
/// est vérifié en défense en profondeur : le noyau, les zones réservées et les
/// allocations permanentes n'y figurent jamais.
pub fn free(range: FrameRange) -> Result<(), PmmError> {
    let _pmm = PMM_LOCK.lock();
    let frames = range.first..range.first + range.count;
    if !frames
        .clone()
        .all(|frame| is_freeable(frame) && !is_free(frame))
    {
        return Err(PmmError::NotFreeable);
    }
    for frame in frames {
        set_freeable(frame, false);
        mark_free(frame);
    }
    // SAFETY: état du PMM, sous PMM_LOCK.
    unsafe {
        if range.first < NEXT_HINT {
            NEXT_HINT = range.first;
        }
    }
    Ok(())
}

/// Efface `count` pages à partir de la page `first` et retourne son adresse.
/// Appelé verrou relâché : les pages appartiennent déjà à l'appelant.
fn zeroed(first: usize, count: usize) -> u64 {
    let start = first as u64 * PAGE_SIZE;
    // SAFETY: pages gérées par le PMM, donc sous la limite de l'identity map,
    // et que personne d'autre ne détient.
    unsafe { write_bytes(start as *mut u8, 0, count * PAGE_SIZE as usize) };
    start
}

/// Recherche next-fit de `count` pages libres contiguës, puis marquage, sous
/// PMM_LOCK. Une page occupée fait repartir la recherche juste après elle.
fn find_and_mark(count: usize, freeable: bool) -> Option<usize> {
    // SAFETY: état du PMM, sous PMM_LOCK (appelants).
    let (managed_frames, hint) = unsafe { (MANAGED_FRAMES, NEXT_HINT) };
    if count == 0 || count > managed_frames {
        return None;
    }
    let first_allocatable = (LOW_MEMORY_RESERVE_END / PAGE_SIZE) as usize;
    let last_start = managed_frames.checked_sub(count)?;
    if first_allocatable > last_start {
        return None;
    }
    let hint = hint.clamp(first_allocatable, last_start);
    let first = scan(hint, last_start, count).or_else(|| scan(first_allocatable, hint, count))?;
    for frame in first..first + count {
        mark_used(frame);
        set_freeable(frame, freeable);
    }
    // SAFETY: état du PMM, sous PMM_LOCK.
    unsafe { NEXT_HINT = first + count };
    Some(first)
}

fn scan(from: usize, to: usize, count: usize) -> Option<usize> {
    let mut start = from;
    while start <= to {
        match (start..start + count).find(|&frame| !is_free(frame)) {
            // Toute fenêtre contenant cette page occupée échoue aussi.
            Some(used) => start = used + 1,
            None => return Some(start),
        }
    }
    None
}

fn is_freeable(frame: usize) -> bool {
    if frame >= MAX_FRAMES {
        return false;
    }
    unsafe {
        let ptr = addr_of_mut!(FREEABLE_BITMAP).cast::<u8>().add(frame / 8);
        (*ptr & (1 << (frame % 8))) != 0
    }
}

fn set_freeable(frame: usize, freeable: bool) {
    if frame >= MAX_FRAMES {
        return;
    }
    unsafe {
        let ptr = addr_of_mut!(FREEABLE_BITMAP).cast::<u8>().add(frame / 8);
        if freeable {
            *ptr |= 1 << (frame % 8);
        } else {
            *ptr &= !(1 << (frame % 8));
        }
    }
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
