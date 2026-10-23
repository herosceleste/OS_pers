use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

pub const MAX_TASKS: usize = 256;
pub const NO_CPU: u32 = u32::MAX;

const STATE_EMPTY: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_RUNNING: u8 = 2;
const STATE_BLOCKED: u8 = 3;
const STATE_SLEEPING: u8 = 4;
const STATE_DEAD: u8 = 5;
const STATE_ALLOCATING: u8 = 0xFF;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static CREATED_TASKS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TaskId(u64);

impl TaskId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Sleeping,
    Dead,
}

impl TaskState {
    #[expect(
        dead_code,
        reason = "API du modèle de tâche K004-01, pas encore consommée au boot"
    )]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Running => "RUNNING",
            Self::Blocked => "BLOCKED",
            Self::Sleeping => "SLEEPING",
            Self::Dead => "DEAD",
        }
    }

    const fn raw(self) -> u8 {
        match self {
            Self::Ready => STATE_READY,
            Self::Running => STATE_RUNNING,
            Self::Blocked => STATE_BLOCKED,
            Self::Sleeping => STATE_SLEEPING,
            Self::Dead => STATE_DEAD,
        }
    }

    const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            STATE_READY => Some(Self::Ready),
            STATE_RUNNING => Some(Self::Running),
            STATE_BLOCKED => Some(Self::Blocked),
            STATE_SLEEPING => Some(Self::Sleeping),
            STATE_DEAD => Some(Self::Dead),
            _ => None,
        }
    }
}

struct TaskSlot {
    id: AtomicU64,
    state: AtomicU8,
    affinity_mask: AtomicU64,
    current_cpu: AtomicU32,
    stack_start: AtomicU64,
    stack_end: AtomicU64,
    saved_rsp: AtomicU64,
}

impl TaskSlot {
    const fn empty() -> Self {
        Self {
            id: AtomicU64::new(0),
            state: AtomicU8::new(STATE_EMPTY),
            affinity_mask: AtomicU64::new(0),
            current_cpu: AtomicU32::new(NO_CPU),
            stack_start: AtomicU64::new(0),
            stack_end: AtomicU64::new(0),
            saved_rsp: AtomicU64::new(0),
        }
    }
}

static TASKS: [TaskSlot; MAX_TASKS] = [const { TaskSlot::empty() }; MAX_TASKS];

#[derive(Clone, Copy)]
pub struct TaskSnapshot {
    #[expect(
        dead_code,
        reason = "API du modèle de tâche K004-01, pas encore consommée au boot"
    )]
    pub id: TaskId,
    pub state: TaskState,
    #[expect(
        dead_code,
        reason = "API du modèle de tâche K004-01, pas encore consommée au boot"
    )]
    pub affinity_mask: u64,
    pub current_cpu: u32,
}

#[derive(Clone, Copy)]
pub enum TaskError {
    CapacityExceeded,
    InvalidAffinity,
    UnknownTask,
    InvalidTransition,
    CpuOutsideAffinity,
    StackAlreadyAttached,
    StackInUse,
}

impl TaskError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CapacityExceeded => "task registry capacity exceeded",
            Self::InvalidAffinity => "task affinity mask is empty",
            Self::UnknownTask => "unknown task id",
            Self::InvalidTransition => "illegal task state transition",
            Self::CpuOutsideAffinity => "CPU is outside task affinity mask",
            Self::StackAlreadyAttached => "task already owns a stack",
            Self::StackInUse => "stack overlaps the stack of another active task",
        }
    }
}

pub struct ModelQualification {
    pub first_id: u64,
    pub second_id: u64,
    pub unique_ids: bool,
    pub successful_transitions: u64,
    pub invalid_transitions_rejected: u64,
    pub states_seen_mask: u8,
    pub created_total: u64,
}

pub fn create_kernel(affinity_mask: u64) -> Result<TaskId, TaskError> {
    if affinity_mask == 0 {
        return Err(TaskError::InvalidAffinity);
    }

    for slot in &TASKS {
        if slot
            .state
            .compare_exchange(
                STATE_EMPTY,
                STATE_ALLOCATING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            continue;
        }

        let id_raw = NEXT_TASK_ID.fetch_add(1, Ordering::AcqRel);
        if id_raw == 0 {
            slot.state.store(STATE_EMPTY, Ordering::Release);
            return Err(TaskError::CapacityExceeded);
        }

        slot.id.store(id_raw, Ordering::Release);
        slot.affinity_mask.store(affinity_mask, Ordering::Release);
        slot.current_cpu.store(NO_CPU, Ordering::Release);
        slot.stack_start.store(0, Ordering::Release);
        slot.stack_end.store(0, Ordering::Release);
        slot.saved_rsp.store(0, Ordering::Release);
        slot.state.store(STATE_READY, Ordering::Release);
        CREATED_TASKS.fetch_add(1, Ordering::AcqRel);

        return Ok(TaskId(id_raw));
    }

    Err(TaskError::CapacityExceeded)
}

pub fn snapshot(id: TaskId) -> Result<TaskSnapshot, TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    let raw = slot.state.load(Ordering::Acquire);
    let state = TaskState::from_raw(raw).ok_or(TaskError::UnknownTask)?;

    Ok(TaskSnapshot {
        id,
        state,
        affinity_mask: slot.affinity_mask.load(Ordering::Acquire),
        current_cpu: slot.current_cpu.load(Ordering::Acquire),
    })
}

pub fn start(id: TaskId, cpu_id: u32) -> Result<(), TaskError> {
    if cpu_id >= 64 {
        return Err(TaskError::CpuOutsideAffinity);
    }

    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    let affinity = slot.affinity_mask.load(Ordering::Acquire);
    if affinity & (1u64 << cpu_id) == 0 {
        return Err(TaskError::CpuOutsideAffinity);
    }

    transition(slot, TaskState::Ready, TaskState::Running)?;
    slot.current_cpu.store(cpu_id, Ordering::Release);
    Ok(())
}

pub fn yield_ready(id: TaskId) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    transition(slot, TaskState::Running, TaskState::Ready)?;
    slot.current_cpu.store(NO_CPU, Ordering::Release);
    Ok(())
}

pub fn block(id: TaskId) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    transition(slot, TaskState::Running, TaskState::Blocked)?;
    slot.current_cpu.store(NO_CPU, Ordering::Release);
    Ok(())
}

pub fn sleep(id: TaskId) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    transition(slot, TaskState::Running, TaskState::Sleeping)?;
    slot.current_cpu.store(NO_CPU, Ordering::Release);
    Ok(())
}

pub fn wake(id: TaskId) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    let current = slot.state.load(Ordering::Acquire);

    if current != STATE_BLOCKED && current != STATE_SLEEPING {
        return Err(TaskError::InvalidTransition);
    }

    slot.state
        .compare_exchange(current, STATE_READY, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| TaskError::InvalidTransition)?;
    slot.current_cpu.store(NO_CPU, Ordering::Release);
    Ok(())
}

pub fn terminate(id: TaskId) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    let current = slot.state.load(Ordering::Acquire);

    if !matches!(
        current,
        STATE_READY | STATE_RUNNING | STATE_BLOCKED | STATE_SLEEPING
    ) {
        return Err(TaskError::InvalidTransition);
    }

    slot.state
        .compare_exchange(current, STATE_DEAD, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| TaskError::InvalidTransition)?;
    slot.current_cpu.store(NO_CPU, Ordering::Release);
    Ok(())
}

/// Attribue la pile `[start, end)` à une tâche READY qui n'en possède pas encore.
///
/// Invariant K004-02 : une pile n'appartient qu'à une seule tâche active. Une
/// pile qui chevauche celle d'une tâche non DEAD est refusée. L'attribution est
/// faite par le BSP pendant le boot, avant que les tâches ne s'exécutent.
pub fn attach_stack(id: TaskId, start: u64, end: u64) -> Result<(), TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    if slot.state.load(Ordering::Acquire) != STATE_READY {
        return Err(TaskError::InvalidTransition);
    }
    if slot.stack_end.load(Ordering::Acquire) != 0 {
        return Err(TaskError::StackAlreadyAttached);
    }

    for other in &TASKS {
        let state = other.state.load(Ordering::Acquire);
        if matches!(state, STATE_EMPTY | STATE_ALLOCATING | STATE_DEAD) {
            continue;
        }
        let other_start = other.stack_start.load(Ordering::Acquire);
        let other_end = other.stack_end.load(Ordering::Acquire);
        if other_end != 0 && start < other_end && other_start < end {
            return Err(TaskError::StackInUse);
        }
    }

    slot.stack_start.store(start, Ordering::Release);
    slot.stack_end.store(end, Ordering::Release);
    Ok(())
}

/// Pile `[start, end)` attribuée à la tâche, si elle en possède une.
pub fn stack_of(id: TaskId) -> Result<Option<(u64, u64)>, TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    let end = slot.stack_end.load(Ordering::Acquire);
    if end == 0 {
        return Ok(None);
    }
    Ok(Some((slot.stack_start.load(Ordering::Acquire), end)))
}

/// Identifiant d'une tâche existante, retrouvé à partir de sa valeur brute.
pub fn lookup(raw: u64) -> Result<TaskId, TaskError> {
    let id = TaskId(raw);
    find_slot(id).map(|_| id).ok_or(TaskError::UnknownTask)
}

/// Emplacement du RSP sauvegardé de la tâche, écrit et relu par le switch de
/// contexte. Le pointeur reste valide tant que le registre statique existe.
pub fn saved_rsp_ptr(id: TaskId) -> Result<*mut u64, TaskError> {
    let slot = find_slot(id).ok_or(TaskError::UnknownTask)?;
    Ok(slot.saved_rsp.as_ptr())
}

/// Nombre de tâches actuellement RUNNING, tous CPU confondus.
pub fn running_count() -> usize {
    TASKS
        .iter()
        .filter(|slot| slot.state.load(Ordering::Acquire) == STATE_RUNNING)
        .count()
}

pub fn created_total() -> u64 {
    CREATED_TASKS.load(Ordering::Acquire)
}

pub fn qualify_model() -> Result<ModelQualification, TaskError> {
    let affinity_all = 0b1111u64;
    let first = create_kernel(affinity_all)?;
    let second = create_kernel(0b0001)?;

    let unique_ids = first != second;
    if !unique_ids {
        return Err(TaskError::InvalidTransition);
    }

    let mut states_seen = state_bit(TaskState::Ready);
    let mut transitions = 0u64;
    let mut rejected = 0u64;

    start(first, 0)?;
    transitions += 1;
    states_seen |= state_bit(TaskState::Running);

    yield_ready(first)?;
    transitions += 1;

    start(first, 1)?;
    transitions += 1;

    block(first)?;
    transitions += 1;
    states_seen |= state_bit(TaskState::Blocked);

    wake(first)?;
    transitions += 1;

    start(first, 2)?;
    transitions += 1;

    sleep(first)?;
    transitions += 1;
    states_seen |= state_bit(TaskState::Sleeping);

    wake(first)?;
    transitions += 1;

    start(first, 3)?;
    transitions += 1;

    terminate(first)?;
    transitions += 1;
    states_seen |= state_bit(TaskState::Dead);

    match block(second) {
        Err(TaskError::InvalidTransition) => rejected += 1,
        Err(error) => return Err(error),
        Ok(()) => return Err(TaskError::InvalidTransition),
    }

    match start(second, 1) {
        Err(TaskError::CpuOutsideAffinity) => rejected += 1,
        Err(error) => return Err(error),
        Ok(()) => return Err(TaskError::InvalidTransition),
    }

    start(second, 0)?;
    transitions += 1;
    terminate(second)?;
    transitions += 1;

    let first_snapshot = snapshot(first)?;
    let second_snapshot = snapshot(second)?;

    if first_snapshot.state != TaskState::Dead
        || second_snapshot.state != TaskState::Dead
        || first_snapshot.current_cpu != NO_CPU
        || second_snapshot.current_cpu != NO_CPU
    {
        return Err(TaskError::InvalidTransition);
    }

    Ok(ModelQualification {
        first_id: first.raw(),
        second_id: second.raw(),
        unique_ids,
        successful_transitions: transitions,
        invalid_transitions_rejected: rejected,
        states_seen_mask: states_seen,
        created_total: created_total(),
    })
}

pub const fn required_states_mask() -> u8 {
    state_bit(TaskState::Ready)
        | state_bit(TaskState::Running)
        | state_bit(TaskState::Blocked)
        | state_bit(TaskState::Sleeping)
        | state_bit(TaskState::Dead)
}

const fn state_bit(state: TaskState) -> u8 {
    1 << (state.raw() - 1)
}

fn find_slot(id: TaskId) -> Option<&'static TaskSlot> {
    if id.raw() == 0 {
        return None;
    }

    TASKS.iter().find(|slot| {
        let state = slot.state.load(Ordering::Acquire);
        state != STATE_EMPTY
            && state != STATE_ALLOCATING
            && slot.id.load(Ordering::Acquire) == id.raw()
    })
}

fn transition(slot: &TaskSlot, expected: TaskState, next: TaskState) -> Result<(), TaskError> {
    slot.state
        .compare_exchange(
            expected.raw(),
            next.raw(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map(|_| ())
        .map_err(|_| TaskError::InvalidTransition)
}
