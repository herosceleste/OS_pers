use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use elyon_abi::PAGE_SIZE;

use crate::acpi::{AcpiTopology, CpuDescriptor};
use crate::arch::x86_64::{MAX_CPU_SLOTS, cpu, gdt};
use crate::memory;

const AP_KERNEL_STACK_FRAMES: usize = 16;
const AP_IST_FRAMES: usize = 4;
const INVALID_CPU_INDEX: usize = usize::MAX;

const STATE_EMPTY: u8 = 0;
const STATE_PREPARED: u8 = 1;
const STATE_ONLINE: u8 = 2;

struct CpuSlot {
    logical_id: AtomicU32,
    apic_id: AtomicU32,
    state: AtomicU8,
    kernel_stack_start: AtomicU64,
    kernel_stack_end: AtomicU64,
    gdt_address: AtomicU64,
    tss_address: AtomicU64,
    ist_start: AtomicU64,
    ist_end: AtomicU64,
    ticks: AtomicU64,
    heartbeat: AtomicU64,
    boot_count: AtomicU32,
}

impl CpuSlot {
    const fn empty() -> Self {
        Self {
            logical_id: AtomicU32::new(u32::MAX),
            apic_id: AtomicU32::new(u32::MAX),
            state: AtomicU8::new(STATE_EMPTY),
            kernel_stack_start: AtomicU64::new(0),
            kernel_stack_end: AtomicU64::new(0),
            gdt_address: AtomicU64::new(0),
            tss_address: AtomicU64::new(0),
            ist_start: AtomicU64::new(0),
            ist_end: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            heartbeat: AtomicU64::new(0),
            boot_count: AtomicU32::new(0),
        }
    }

    fn clear(&self) {
        self.logical_id.store(u32::MAX, Ordering::Release);
        self.apic_id.store(u32::MAX, Ordering::Release);
        self.state.store(STATE_EMPTY, Ordering::Release);
        self.kernel_stack_start.store(0, Ordering::Release);
        self.kernel_stack_end.store(0, Ordering::Release);
        self.gdt_address.store(0, Ordering::Release);
        self.tss_address.store(0, Ordering::Release);
        self.ist_start.store(0, Ordering::Release);
        self.ist_end.store(0, Ordering::Release);
        self.ticks.store(0, Ordering::Release);
        self.heartbeat.store(0, Ordering::Release);
        self.boot_count.store(0, Ordering::Release);
    }
}

static CPU_SLOTS: [CpuSlot; MAX_CPU_SLOTS] = [const { CpuSlot::empty() }; MAX_CPU_SLOTS];
static CPU_COUNT: AtomicUsize = AtomicUsize::new(0);
static BSP_INDEX: AtomicUsize = AtomicUsize::new(INVALID_CPU_INDEX);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CpuState {
    Prepared,
    Online,
}

impl CpuState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Online => "online",
        }
    }

    const fn raw(self) -> u8 {
        match self {
            Self::Prepared => STATE_PREPARED,
            Self::Online => STATE_ONLINE,
        }
    }
}

#[derive(Clone, Copy)]
pub struct CpuSnapshot {
    pub logical_id: u32,
    pub apic_id: u32,
    pub state: CpuState,
    pub kernel_stack_start: u64,
    pub kernel_stack_end: u64,
    pub gdt_address: u64,
    pub tss_address: u64,
    pub ist_start: u64,
    pub ist_end: u64,
    pub ticks: u64,
    pub heartbeat: u64,
    pub boot_count: u32,
}

pub struct PerCpuSummary {
    pub cpu_count: usize,
    pub bsp_index: usize,
    pub bsp_apic_id: u32,
    pub resources_unique: bool,
    pub resources_owned: bool,
    pub bsp_tss_loaded: bool,
}

#[derive(Clone, Copy)]
pub enum PerCpuError {
    NoUsableCpu,
    CapacityExceeded,
    BspNotFound,
    DuplicateApicId,
    InvalidBspStack,
    AllocationFailed,
    ResourceOutsideIdentityMap,
    StaticResourceOutsideKernel,
    ResourceOverlap,
    BspTssNotLoaded,
}

impl PerCpuError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoUsableCpu => "MADT exposes no usable CPU",
            Self::CapacityExceeded => "per-CPU capacity exceeded",
            Self::BspNotFound => "BSP APIC ID absent from usable MADT CPUs",
            Self::DuplicateApicId => "duplicate usable APIC ID in MADT",
            Self::InvalidBspStack => "invalid loader-provided BSP kernel stack",
            Self::AllocationFailed => "PMM could not allocate per-CPU stack or IST",
            Self::ResourceOutsideIdentityMap => "per-CPU resource outside current identity map",
            Self::StaticResourceOutsideKernel => "per-CPU GDT/TSS outside kernel image",
            Self::ResourceOverlap => "per-CPU resources overlap",
            Self::BspTssNotLoaded => "BSP task register does not reference slot 0 TSS",
        }
    }
}

pub fn prepare(
    topology: &AcpiTopology,
    bsp_apic_id: u32,
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    kernel_start: u64,
    kernel_end: u64,
    identity_limit: u64,
) -> Result<PerCpuSummary, PerCpuError> {
    for slot in &CPU_SLOTS {
        slot.clear();
    }

    if bsp_stack_start == 0 || bsp_stack_end <= bsp_stack_start {
        return Err(PerCpuError::InvalidBspStack);
    }

    let usable_count = topology
        .cpus()
        .iter()
        .filter(|entry| entry.usable())
        .count();
    if usable_count == 0 {
        return Err(PerCpuError::NoUsableCpu);
    }
    if usable_count > MAX_CPU_SLOTS {
        return Err(PerCpuError::CapacityExceeded);
    }

    validate_unique_apic_ids(topology)?;

    let bsp = topology
        .cpus()
        .iter()
        .find(|entry| entry.usable() && entry.apic_id == bsp_apic_id)
        .copied()
        .ok_or(PerCpuError::BspNotFound)?;

    configure_slot(
        0,
        bsp,
        CpuState::Online,
        bsp_stack_start,
        bsp_stack_end,
        gdt::double_fault_stack_span_for(0),
    );

    let expected_tr = gdt::tss_selector_for(0);
    let bsp_tss_loaded = expected_tr != 0 && cpu::read_tr() == expected_tr;
    if !bsp_tss_loaded {
        return Err(PerCpuError::BspTssNotLoaded);
    }

    let mut logical = 1usize;
    for descriptor in topology.cpus() {
        if !descriptor.usable() || descriptor.apic_id == bsp_apic_id {
            continue;
        }

        let stack_start = memory::allocate_contiguous_frames(AP_KERNEL_STACK_FRAMES)
            .ok_or(PerCpuError::AllocationFailed)?;
        let stack_end = stack_start + AP_KERNEL_STACK_FRAMES as u64 * PAGE_SIZE;

        let ist_start = memory::allocate_contiguous_frames(AP_IST_FRAMES)
            .ok_or(PerCpuError::AllocationFailed)?;
        let ist_end = ist_start + AP_IST_FRAMES as u64 * PAGE_SIZE;

        if stack_end > identity_limit || ist_end > identity_limit {
            return Err(PerCpuError::ResourceOutsideIdentityMap);
        }

        gdt::prepare_for_cpu(logical, ist_start, ist_end);

        configure_slot(
            logical,
            *descriptor,
            CpuState::Prepared,
            stack_start,
            stack_end,
            (ist_start, ist_end),
        );
        logical += 1;
    }

    CPU_COUNT.store(logical, Ordering::Release);
    BSP_INDEX.store(0, Ordering::Release);

    let resources_owned = validate_ownership(logical, kernel_start, kernel_end, identity_limit);
    if !resources_owned {
        return Err(PerCpuError::StaticResourceOutsideKernel);
    }

    let resources_unique = validate_resource_uniqueness(logical);
    if !resources_unique {
        return Err(PerCpuError::ResourceOverlap);
    }

    Ok(PerCpuSummary {
        cpu_count: logical,
        bsp_index: 0,
        bsp_apic_id,
        resources_unique,
        resources_owned,
        bsp_tss_loaded,
    })
}

pub fn cpu_count() -> usize {
    CPU_COUNT.load(Ordering::Acquire)
}

pub fn snapshot(index: usize) -> Option<CpuSnapshot> {
    if index >= cpu_count() {
        return None;
    }

    let slot = &CPU_SLOTS[index];
    let state = match slot.state.load(Ordering::Acquire) {
        STATE_PREPARED => CpuState::Prepared,
        STATE_ONLINE => CpuState::Online,
        _ => return None,
    };

    Some(CpuSnapshot {
        logical_id: slot.logical_id.load(Ordering::Acquire),
        apic_id: slot.apic_id.load(Ordering::Acquire),
        state,
        kernel_stack_start: slot.kernel_stack_start.load(Ordering::Acquire),
        kernel_stack_end: slot.kernel_stack_end.load(Ordering::Acquire),
        gdt_address: slot.gdt_address.load(Ordering::Acquire),
        tss_address: slot.tss_address.load(Ordering::Acquire),
        ist_start: slot.ist_start.load(Ordering::Acquire),
        ist_end: slot.ist_end.load(Ordering::Acquire),
        ticks: slot.ticks.load(Ordering::Acquire),
        heartbeat: slot.heartbeat.load(Ordering::Acquire),
        boot_count: slot.boot_count.load(Ordering::Acquire),
    })
}

pub fn reset_bsp_ticks() {
    let index = BSP_INDEX.load(Ordering::Acquire);
    if index < MAX_CPU_SLOTS {
        CPU_SLOTS[index].ticks.store(0, Ordering::Release);
    }
}

pub fn increment_bsp_ticks() {
    let index = BSP_INDEX.load(Ordering::Acquire);
    if index < MAX_CPU_SLOTS {
        CPU_SLOTS[index].ticks.fetch_add(1, Ordering::AcqRel);
    }
}

pub fn bsp_ticks() -> u64 {
    let index = BSP_INDEX.load(Ordering::Acquire);
    if index < MAX_CPU_SLOTS {
        CPU_SLOTS[index].ticks.load(Ordering::Acquire)
    } else {
        0
    }
}

fn configure_slot(
    logical_id: usize,
    descriptor: CpuDescriptor,
    state: CpuState,
    stack_start: u64,
    stack_end: u64,
    ist_span: (u64, u64),
) {
    let slot = &CPU_SLOTS[logical_id];

    slot.logical_id.store(logical_id as u32, Ordering::Release);
    slot.apic_id.store(descriptor.apic_id, Ordering::Release);
    slot.state.store(state.raw(), Ordering::Release);
    slot.kernel_stack_start
        .store(stack_start, Ordering::Release);
    slot.kernel_stack_end.store(stack_end, Ordering::Release);
    slot.gdt_address
        .store(gdt::gdt_address(logical_id), Ordering::Release);
    slot.tss_address
        .store(gdt::tss_address(logical_id), Ordering::Release);
    slot.ist_start.store(ist_span.0, Ordering::Release);
    slot.ist_end.store(ist_span.1, Ordering::Release);
    slot.ticks.store(0, Ordering::Release);
    slot.heartbeat.store(0, Ordering::Release);
    slot.boot_count.store(0, Ordering::Release);
}

fn validate_unique_apic_ids(topology: &AcpiTopology) -> Result<(), PerCpuError> {
    for (index, left) in topology.cpus().iter().enumerate() {
        if !left.usable() {
            continue;
        }

        for right in &topology.cpus()[index + 1..] {
            if right.usable() && left.apic_id == right.apic_id {
                return Err(PerCpuError::DuplicateApicId);
            }
        }
    }

    Ok(())
}

fn validate_ownership(
    count: usize,
    kernel_start: u64,
    kernel_end: u64,
    identity_limit: u64,
) -> bool {
    if kernel_end <= kernel_start {
        return false;
    }

    for index in 0..count {
        let Some(cpu) = snapshot(index) else {
            return false;
        };

        if cpu.gdt_address < kernel_start || cpu.gdt_address >= kernel_end {
            return false;
        }
        if cpu.tss_address < kernel_start || cpu.tss_address >= kernel_end {
            return false;
        }

        if cpu.kernel_stack_start == 0
            || cpu.kernel_stack_end <= cpu.kernel_stack_start
            || cpu.kernel_stack_end > identity_limit
            || cpu.ist_start == 0
            || cpu.ist_end <= cpu.ist_start
            || cpu.ist_end > identity_limit
        {
            return false;
        }

        if index == 0 {
            if !range_inside(cpu.ist_start, cpu.ist_end, kernel_start, kernel_end) {
                return false;
            }
        } else if ranges_overlap(
            cpu.kernel_stack_start,
            cpu.kernel_stack_end,
            kernel_start,
            kernel_end,
        ) || ranges_overlap(cpu.ist_start, cpu.ist_end, kernel_start, kernel_end)
        {
            return false;
        }
    }

    true
}

fn validate_resource_uniqueness(count: usize) -> bool {
    for left_index in 0..count {
        let Some(left) = snapshot(left_index) else {
            return false;
        };

        for right_index in left_index + 1..count {
            let Some(right) = snapshot(right_index) else {
                return false;
            };

            if ranges_overlap(
                left.kernel_stack_start,
                left.kernel_stack_end,
                right.kernel_stack_start,
                right.kernel_stack_end,
            ) || ranges_overlap(left.ist_start, left.ist_end, right.ist_start, right.ist_end)
                || ranges_overlap(
                    left.kernel_stack_start,
                    left.kernel_stack_end,
                    right.ist_start,
                    right.ist_end,
                )
                || ranges_overlap(
                    left.ist_start,
                    left.ist_end,
                    right.kernel_stack_start,
                    right.kernel_stack_end,
                )
                || left.gdt_address == right.gdt_address
                || left.tss_address == right.tss_address
            {
                return false;
            }
        }
    }

    true
}

fn range_inside(start: u64, end: u64, owner_start: u64, owner_end: u64) -> bool {
    start >= owner_start && end > start && end <= owner_end
}

fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

#[derive(Clone, Copy)]
pub enum OnlineError {
    InvalidIndex,
    NotPrepared,
    ApicIdMismatch,
}

impl OnlineError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidIndex => "invalid per-CPU index",
            Self::NotPrepared => "per-CPU slot is not prepared",
            Self::ApicIdMismatch => "observed APIC ID does not match per-CPU slot",
        }
    }
}

pub fn mark_online(index: usize, observed_apic_id: u32) -> Result<(), OnlineError> {
    if index >= cpu_count() {
        return Err(OnlineError::InvalidIndex);
    }

    let slot = &CPU_SLOTS[index];
    if slot.state.load(Ordering::Acquire) != STATE_PREPARED {
        return Err(OnlineError::NotPrepared);
    }
    if slot.apic_id.load(Ordering::Acquire) != observed_apic_id {
        return Err(OnlineError::ApicIdMismatch);
    }

    slot.boot_count.fetch_add(1, Ordering::AcqRel);
    slot.state.store(STATE_ONLINE, Ordering::Release);
    Ok(())
}

pub fn is_online(index: usize) -> bool {
    index < cpu_count() && CPU_SLOTS[index].state.load(Ordering::Acquire) == STATE_ONLINE
}

pub fn online_count() -> usize {
    (0..cpu_count()).filter(|index| is_online(*index)).count()
}

pub fn prepared_count() -> usize {
    (0..cpu_count())
        .filter(|index| CPU_SLOTS[*index].state.load(Ordering::Acquire) == STATE_PREPARED)
        .count()
}


pub fn record_heartbeat_by_apic_id(apic_id: u32) -> bool {
    for index in 1..cpu_count() {
        let slot = &CPU_SLOTS[index];
        if slot.state.load(Ordering::Acquire) == STATE_ONLINE
            && slot.apic_id.load(Ordering::Acquire) == apic_id
        {
            slot.heartbeat.fetch_add(1, Ordering::AcqRel);
            return true;
        }
    }

    false
}
