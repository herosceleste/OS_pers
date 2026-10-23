//! K004-03 : Round-Robin coopératif avec `yield_now()` et run queue.
//!
//! Le BSP place trois tâches dans une run queue FIFO puis donne la main à la
//! première. Chaque tâche note son passage dans une trace puis appelle
//! `yield_now()` : elle repasse en fin de file et le switch se fait directement
//! vers la tâche suivante, sans repasser par le BSP. Après sa dernière itération
//! une tâche appelle `exit_now()` ; la dernière sortie rend la main au BSP, qui
//! vérifie la rotation, les états et les invariants.
//!
//! Tout se déroule sur le BSP, interruptions masquées : la file n'a pas besoin
//! de verrou à ce stade (K004-07 et K004-10 traiteront le per-CPU et le SMP).

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use elyon_core::preempt::{self, Quantum, SliceClock, TickDecision};
use elyon_core::run_queue::{self, QueueError, RunQueue};
use elyon_core::task_stack::{self, StackRegion};

use crate::arch::x86_64::cpu;
use crate::task::{self, TaskError, TaskState};
use crate::task_stack::{self as stacks, TaskStackError};
use crate::timer::{self, TimerError};
use crate::{context, kprintln};

pub const QUALIFY_TASKS: usize = 3;
pub const ITERATIONS: u64 = 4;
pub const TRACE_CAPACITY: usize = QUALIFY_TASKS * ITERATIONS as usize;

const QUEUE_CAPACITY: usize = 8;
const ARG_BASE: u64 = 0xE1_0403_0000;
const BSP_CPU: u32 = 0;
const BSP_AFFINITY: u64 = 1 << BSP_CPU;
/// Valeur de `CURRENT` lorsque le BSP, et non une tâche, détient le CPU.
const BSP_CONTEXT: u64 = 0;

const EXPECTED_YIELDS: u64 = QUALIFY_TASKS as u64 * (ITERATIONS - 1);
const EXPECTED_EXITS: u64 = QUALIFY_TASKS as u64;
/// Chaque passage de la trace succède à un switch, sauf le premier (depuis le BSP).
const EXPECTED_TASK_SWITCHES: u64 = TRACE_CAPACITY as u64 - 1;
/// BSP → première tâche, puis dernière sortie → BSP.
const EXPECTED_BSP_SWITCHES: u64 = 2;

static mut QUEUE: RunQueue<QUEUE_CAPACITY> = RunQueue::new();
static mut BSP_RSP: u64 = 0;

static CURRENT: AtomicU64 = AtomicU64::new(BSP_CONTEXT);
static IDS: [AtomicU64; QUALIFY_TASKS] = [const { AtomicU64::new(0) }; QUALIFY_TASKS];
static TRACE: [AtomicU64; TRACE_CAPACITY] = [const { AtomicU64::new(0) }; TRACE_CAPACITY];
static TRACE_LEN: AtomicUsize = AtomicUsize::new(0);
static TASK_SWITCHES: AtomicU64 = AtomicU64::new(0);
static BSP_SWITCHES: AtomicU64 = AtomicU64::new(0);
static YIELDS: AtomicU64 = AtomicU64::new(0);
static EXITS: AtomicU64 = AtomicU64::new(0);
static RUNNING_MAX: AtomicUsize = AtomicUsize::new(0);
static ARGS_OK: AtomicU64 = AtomicU64::new(0);
static RSP_OK: AtomicU64 = AtomicU64::new(0);

pub struct SchedQualification {
    pub ids: [u64; QUALIFY_TASKS],
    pub trace: [u64; TRACE_CAPACITY],
    pub yields: u64,
    pub exits: u64,
    pub task_switches: u64,
    pub bsp_switches: u64,
    pub running_max: usize,
    pub args_ok: u64,
    pub rsp_ok: u64,
    pub canaries_ok: usize,
}

#[derive(Clone, Copy)]
pub enum SchedError {
    Task(TaskError),
    Stack(TaskStackError),
    Queue(QueueError),
    Timer(TimerError),
    EmptyQueue,
    NotInTaskContext,
    NotBackOnBsp,
    UnknownCurrentTask,
    TraceOverflow,
    ArgMismatch,
    RspOutsideOwnStack,
    RunningInvariant,
    NotRoundRobin,
    TaskNotDead,
    QueueNotEmpty,
    CountMismatch,
    CanaryCorrupted,
    BspRspOutsideKernelStack,
    TooFewPreemptions,
    TooFewSlices,
    NoProgress,
    InterruptsLeftEnabled,
}

impl SchedError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task(error) => error.as_str(),
            Self::Stack(error) => error.as_str(),
            Self::Queue(error) => error.as_str(),
            Self::Timer(error) => error.as_str(),
            Self::EmptyQueue => "run queue empty when a task was expected",
            Self::NotInTaskContext => "yield or exit called outside a task",
            Self::NotBackOnBsp => "BSP resumed while a task is still current",
            Self::UnknownCurrentTask => "current task is not one of the qualified tasks",
            Self::TraceOverflow => "scheduler trace overflow",
            Self::ArgMismatch => "task received an unexpected entry argument",
            Self::RspOutsideOwnStack => "task RSP outside its own stack",
            Self::RunningInvariant => "more than one task RUNNING, or a task RUNNING on the BSP",
            Self::NotRoundRobin => "execution trace is not a strict round-robin",
            Self::TaskNotDead => "task not DEAD after the cooperative run",
            Self::QueueNotEmpty => "run queue not empty after the cooperative run",
            Self::CountMismatch => "unexpected yield, exit or switch count",
            Self::CanaryCorrupted => "task stack canary overwritten",
            Self::BspRspOutsideKernelStack => "saved BSP RSP outside loader-provided kernel stack",
            Self::TooFewPreemptions => "timer did not preempt the tasks often enough",
            Self::TooFewSlices => "a task received fewer slices than its fair share",
            Self::NoProgress => "a preemptible task made no progress",
            Self::InterruptsLeftEnabled => "BSP resumed with interrupts enabled",
        }
    }
}

impl From<TaskError> for SchedError {
    fn from(error: TaskError) -> Self {
        Self::Task(error)
    }
}

impl From<QueueError> for SchedError {
    fn from(error: QueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<TimerError> for SchedError {
    fn from(error: TimerError) -> Self {
        Self::Timer(error)
    }
}

impl From<TaskStackError> for SchedError {
    fn from(error: TaskStackError) -> Self {
        Self::Stack(error)
    }
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<SchedQualification, SchedError> {
    cpu::disable_interrupts();

    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let entry = elyon_sched_task_entry as *const () as usize as u64;

    reset();

    let mut regions = [StackRegion { start: 0, end: 0 }; QUALIFY_TASKS];
    for (index, region) in regions.iter_mut().enumerate() {
        let id = task::create_kernel(BSP_AFFINITY)?;
        let prepared = stacks::prepare(id, entry, ARG_BASE + index as u64, bsp, identity_limit)?;
        // SAFETY: l'emplacement appartient au slot statique de la tâche, qui n'a
        // encore jamais été exécutée ; seul le switch le relira.
        unsafe { write_volatile(task::saved_rsp_ptr(id)?, prepared.initial_rsp) };
        IDS[index].store(id.raw(), Ordering::Release);
        *region = prepared.region;
        queue().push_back(id.raw())?;
    }

    let first = run_queue::exit_next(queue()).ok_or(SchedError::EmptyQueue)?;
    task::start(task::lookup(first)?, BSP_CPU)?;
    BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(BSP_CONTEXT, first)?;

    // Le BSP ne reprend la main qu'après la sortie de la dernière tâche.
    if CURRENT.load(Ordering::Acquire) != BSP_CONTEXT {
        return Err(SchedError::NotBackOnBsp);
    }

    let mut ids = [0u64; QUALIFY_TASKS];
    for (index, id) in ids.iter_mut().enumerate() {
        *id = IDS[index].load(Ordering::Acquire);
    }

    let trace_len = TRACE_LEN.load(Ordering::Acquire);
    let mut trace = [0u64; TRACE_CAPACITY];
    for (index, entry) in trace.iter_mut().enumerate() {
        *entry = TRACE[index].load(Ordering::Acquire);
    }
    if trace_len != TRACE_CAPACITY || !run_queue::is_round_robin(&trace, &ids, ITERATIONS) {
        return Err(SchedError::NotRoundRobin);
    }

    for &raw in &ids {
        if task::snapshot(task::lookup(raw)?)?.state != TaskState::Dead {
            return Err(SchedError::TaskNotDead);
        }
    }
    if !queue().is_empty() {
        return Err(SchedError::QueueNotEmpty);
    }
    if task::running_count() != 0 || RUNNING_MAX.load(Ordering::Acquire) != 1 {
        return Err(SchedError::RunningInvariant);
    }

    let yields = YIELDS.load(Ordering::Acquire);
    let exits = EXITS.load(Ordering::Acquire);
    let task_switches = TASK_SWITCHES.load(Ordering::Acquire);
    let bsp_switches = BSP_SWITCHES.load(Ordering::Acquire);
    if yields != EXPECTED_YIELDS
        || exits != EXPECTED_EXITS
        || task_switches != EXPECTED_TASK_SWITCHES
        || bsp_switches != EXPECTED_BSP_SWITCHES
    {
        return Err(SchedError::CountMismatch);
    }

    let args_ok = ARGS_OK.load(Ordering::Acquire);
    let rsp_ok = RSP_OK.load(Ordering::Acquire);
    if args_ok != QUALIFY_TASKS as u64 {
        return Err(SchedError::ArgMismatch);
    }
    if rsp_ok != TRACE_CAPACITY as u64 {
        return Err(SchedError::RspOutsideOwnStack);
    }

    let mut canaries_ok = 0;
    for region in &regions {
        if !stacks::canary_intact_at(*region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
    let bsp_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };
    if !task_stack::rsp_inside(bsp, bsp_rsp) {
        return Err(SchedError::BspRspOutsideKernelStack);
    }

    Ok(SchedQualification {
        ids,
        trace,
        yields,
        exits,
        task_switches,
        bsp_switches,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        args_ok,
        rsp_ok,
        canaries_ok,
    })
}

/// Cède le CPU : la tâche courante repasse en fin de run queue et la tâche en
/// tête reprend la main, directement depuis cette tâche.
///
/// La run queue est manipulée IRQ masquées (irq-save) : un tick ne peut pas
/// préempter la tâche au milieu de la mise à jour. L'état d'IF est restauré au
/// retour, quand la tâche reprend la main.
pub fn yield_now() {
    let interrupts = cpu::interrupts_enabled();
    cpu::disable_interrupts();
    if let Err(error) = try_yield() {
        fail(error);
    }
    if interrupts {
        cpu::enable_interrupts();
    }
}

/// Termine la tâche courante : elle passe DEAD et n'est pas remise en file. La
/// tâche suivante reprend la main ; si la file est vide, le BSP la reprend.
///
/// IRQ masquées dès l'entrée : la tâche ne reprend jamais, rien à restaurer.
pub fn exit_now() -> ! {
    cpu::disable_interrupts();
    if let Err(error) = try_exit() {
        fail(error);
    }
    kprintln!("[sched] FAILED task resumed after exit");
    cpu::halt_forever()
}

fn try_yield() -> Result<(), SchedError> {
    let current = CURRENT.load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }

    task::yield_ready(task::lookup(current)?)?;
    let next = run_queue::yield_next(queue(), current)?;
    YIELDS.fetch_add(1, Ordering::AcqRel);
    task::start(task::lookup(next)?, BSP_CPU)?;

    if next != current {
        TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
        transfer(current, next)?;
    }
    Ok(())
}

fn try_exit() -> Result<(), SchedError> {
    let current = CURRENT.load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }

    task::terminate(task::lookup(current)?)?;
    EXITS.fetch_add(1, Ordering::AcqRel);

    match run_queue::exit_next(queue()) {
        Some(next) => {
            task::start(task::lookup(next)?, BSP_CPU)?;
            TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
            transfer(current, next)
        }
        None => {
            BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
            transfer(current, BSP_CONTEXT)
        }
    }
}

/// Passe le CPU du contexte `current` au contexte `next` (`BSP_CONTEXT` pour le
/// BSP). Les transitions d'état sont faites par l'appelant ; l'invariant « une
/// seule tâche RUNNING » est vérifié juste avant le switch.
fn transfer(current: u64, next: u64) -> Result<(), SchedError> {
    let running = task::running_count();
    RUNNING_MAX.fetch_max(running, Ordering::AcqRel);
    let expected_running = if next == BSP_CONTEXT { 0 } else { 1 };
    if running != expected_running {
        return Err(SchedError::RunningInvariant);
    }

    let old_rsp = if current == BSP_CONTEXT {
        addr_of_mut!(BSP_RSP)
    } else {
        task::saved_rsp_ptr(task::lookup(current)?)?
    };
    let new_rsp = if next == BSP_CONTEXT {
        // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
        unsafe { read_volatile(addr_of!(BSP_RSP)) }
    } else {
        // SAFETY: l'emplacement appartient au slot statique de la tâche ; il a
        // été écrit par qualify() ou par le dernier switch qui l'a quittée.
        unsafe { read_volatile(task::saved_rsp_ptr(task::lookup(next)?)?) }
    };

    CURRENT.store(next, Ordering::Release);
    // SAFETY: `new_rsp` désigne le frame initial ou celui laissé par le dernier
    // switch de ce contexte, sur sa propre pile ; aucun verrou n'est tenu.
    unsafe { context::switch(old_rsp, new_rsp) };
    Ok(())
}

fn reset() {
    *queue() = RunQueue::new();
    CURRENT.store(BSP_CONTEXT, Ordering::Release);
    TRACE_LEN.store(0, Ordering::Release);
    for entry in &TRACE {
        entry.store(0, Ordering::Release);
    }
    for counter in [
        &TASK_SWITCHES,
        &BSP_SWITCHES,
        &YIELDS,
        &EXITS,
        &ARGS_OK,
        &RSP_OK,
    ] {
        counter.store(0, Ordering::Release);
    }
    RUNNING_MAX.store(0, Ordering::Release);
}

fn queue() -> &'static mut RunQueue<QUEUE_CAPACITY> {
    // SAFETY: la file n'est manipulée que sur le BSP, interruptions masquées, et
    // aucune référence obtenue ici n'est conservée au-delà de l'appel en cours.
    unsafe { &mut *addr_of_mut!(QUEUE) }
}

fn record(current: u64) -> Result<(), SchedError> {
    let slot = TRACE_LEN.fetch_add(1, Ordering::AcqRel);
    let entry = TRACE.get(slot).ok_or(SchedError::TraceOverflow)?;
    entry.store(current, Ordering::Release);

    let (start, end) =
        task::stack_of(task::lookup(current)?)?.ok_or(SchedError::RspOutsideOwnStack)?;
    if !task_stack::rsp_inside(StackRegion { start, end }, cpu::read_rsp()) {
        return Err(SchedError::RspOutsideOwnStack);
    }
    RSP_OK.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

fn check_arg(current: u64, arg: u64) -> Result<(), SchedError> {
    let index = IDS
        .iter()
        .position(|id| id.load(Ordering::Acquire) == current)
        .ok_or(SchedError::UnknownCurrentTask)?;
    if arg != ARG_BASE + index as u64 {
        return Err(SchedError::ArgMismatch);
    }
    ARGS_OK.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

fn fail(error: SchedError) -> ! {
    kprintln!("[sched] FAILED reason={}", error.as_str());
    cpu::halt_forever()
}

#[unsafe(no_mangle)]
extern "C" fn elyon_sched_task_entry(arg: u64) -> ! {
    let current = CURRENT.load(Ordering::Acquire);
    if let Err(error) = check_arg(current, arg) {
        fail(error);
    }

    for iteration in 0..ITERATIONS {
        if let Err(error) = record(current) {
            fail(error);
        }
        if iteration + 1 < ITERATIONS {
            yield_now();
        }
    }

    exit_now()
}

// --- K004-04 : préemption par timer LAPIC ------------------------------------------
//
// Trois tâches tournent en boucle de calcul pure, sans jamais céder. À chaque
// tick, `on_timer_tick` (appelé par le handler timer après l'EOI) décompte la
// tranche de la tâche courante ; tranche épuisée, la tâche repasse en fin de
// file et le switch se fait depuis l'IRQ, sur sa pile : elle reprendra plus
// tard par son propre `iretq`. Après `PREEMPT_TARGET` préemptions, un drapeau
// d'arrêt est levé et les tâches sortent. La trace n'enregistre que les
// tranches antérieures à l'arrêt : elle compte exactement `PREEMPT_TARGET + 1`
// entrées en rotation stricte, quelle que soit la cadence réelle des ticks.

pub const PREEMPT_TASKS: usize = 3;
pub const PREEMPT_TARGET: u64 = 30;
pub const PREEMPT_TRACE_CAPACITY: usize = PREEMPT_TARGET as usize + 1;
pub const QUANTUM_TICKS: u32 = 1;

const PREEMPT_ARG_BASE: u64 = 0xE1_0404_0000;
const QUANTUM: Quantum = match Quantum::new(QUANTUM_TICKS) {
    Ok(quantum) => quantum,
    Err(_) => panic!("QUANTUM_TICKS must be at least one"),
};
/// Part équitable minimale : la trace de 31 tranches en donne au moins 10 à chacune.
const MIN_FAIR_SLICES: u64 = PREEMPT_TARGET / PREEMPT_TASKS as u64;

static mut CLOCK: SliceClock = SliceClock::new(QUANTUM);

static PREEMPT_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Drapeau d'arrêt de la charge préemptible, lu aussi par l'assembleur de la
/// boucle d'intégrité K004-05 (d'où le nom de symbole stable).
#[unsafe(no_mangle)]
pub static ELYON_SCHED_STOP: AtomicBool = AtomicBool::new(false);
static ACTIVE_ARG_BASE: AtomicU64 = AtomicU64::new(0);
static BUSY: [AtomicBool; PREEMPT_TASKS] = [const { AtomicBool::new(false) }; PREEMPT_TASKS];
static BUSY_PREEMPTIONS: [AtomicU64; PREEMPT_TASKS] = [const { AtomicU64::new(0) }; PREEMPT_TASKS];
static PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
static TASK_TICKS: AtomicU64 = AtomicU64::new(0);
static PREEMPT_IDS: [AtomicU64; PREEMPT_TASKS] = [const { AtomicU64::new(0) }; PREEMPT_TASKS];
static COUNTERS: [AtomicU64; PREEMPT_TASKS] = [const { AtomicU64::new(0) }; PREEMPT_TASKS];
static PREEMPT_TRACE: [AtomicU64; PREEMPT_TRACE_CAPACITY] =
    [const { AtomicU64::new(0) }; PREEMPT_TRACE_CAPACITY];
static PREEMPT_TRACE_LEN: AtomicUsize = AtomicUsize::new(0);

pub struct PreemptQualification {
    pub ids: [u64; PREEMPT_TASKS],
    pub trace: [u64; PREEMPT_TRACE_CAPACITY],
    pub preemptions: u64,
    pub task_ticks: u64,
    pub min_slices: u64,
    pub counters: [u64; PREEMPT_TASKS],
    pub exits: u64,
    pub running_max: usize,
    pub canaries_ok: usize,
}

/// Appelé par le handler timer, IRQ masquées, après l'EOI.
pub fn on_timer_tick() {
    if !PREEMPT_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let current = CURRENT.load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return;
    }
    TASK_TICKS.fetch_add(1, Ordering::AcqRel);
    if clock().on_tick() == TickDecision::Continue {
        return;
    }
    if let Err(error) = preempt_current(current) {
        fail(error);
    }
}

fn preempt_current(current: u64) -> Result<(), SchedError> {
    if let Some(index) = preemptible_index_of(current)
        && BUSY[index].load(Ordering::Acquire)
    {
        BUSY_PREEMPTIONS[index].fetch_add(1, Ordering::AcqRel);
    }
    task::yield_ready(task::lookup(current)?)?;
    let next = run_queue::yield_next(queue(), current)?;
    task::start(task::lookup(next)?, BSP_CPU)?;
    clock().reset();
    if next == current {
        return Ok(());
    }

    let preemptions = PREEMPTIONS.fetch_add(1, Ordering::AcqRel) + 1;
    if !ELYON_SCHED_STOP.load(Ordering::Acquire) {
        record_slice(next)?;
        if preemptions >= PREEMPT_TARGET {
            ELYON_SCHED_STOP.store(true, Ordering::Release);
        }
    }

    TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(current, next)
}

pub fn qualify_preemption(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
) -> Result<PreemptQualification, SchedError> {
    run_preemptive(
        bsp_stack_start,
        bsp_stack_end,
        identity_limit,
        timer_initial_count,
        elyon_preempt_task_entry as *const () as usize as u64,
        PREEMPT_ARG_BASE,
    )
}

/// Fait tourner `PREEMPT_TASKS` tâches d'entrée `entry` (argument
/// `arg_base + index`) sous préemption timer jusqu'à `PREEMPT_TARGET`
/// préemptions, puis vérifie trace, progression, états et invariants.
pub fn run_preemptive(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
    entry: u64,
    arg_base: u64,
) -> Result<PreemptQualification, SchedError> {
    cpu::disable_interrupts();

    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };

    reset();
    reset_preemption();
    ACTIVE_ARG_BASE.store(arg_base, Ordering::Release);

    let mut regions = [StackRegion { start: 0, end: 0 }; PREEMPT_TASKS];
    for (index, region) in regions.iter_mut().enumerate() {
        let id = task::create_kernel(BSP_AFFINITY)?;
        let prepared = stacks::prepare(id, entry, arg_base + index as u64, bsp, identity_limit)?;
        // SAFETY: l'emplacement appartient au slot statique de la tâche, qui n'a
        // encore jamais été exécutée ; seul le switch le relira.
        unsafe { write_volatile(task::saved_rsp_ptr(id)?, prepared.initial_rsp) };
        PREEMPT_IDS[index].store(id.raw(), Ordering::Release);
        *region = prepared.region;
        queue().push_back(id.raw())?;
    }

    let first = run_queue::exit_next(queue()).ok_or(SchedError::EmptyQueue)?;
    task::start(task::lookup(first)?, BSP_CPU)?;
    record_slice(first)?;
    clock().reset();
    PREEMPT_ACTIVE.store(true, Ordering::Release);
    timer::restart_periodic(timer_initial_count)?;

    BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(BSP_CONTEXT, first)?;

    // La dernière tâche sortie rend la main ici, IRQ masquées.
    PREEMPT_ACTIVE.store(false, Ordering::Release);
    let interrupts_left_enabled = cpu::interrupts_enabled();
    cpu::disable_interrupts();
    timer::stop()?;
    if interrupts_left_enabled {
        return Err(SchedError::InterruptsLeftEnabled);
    }
    if CURRENT.load(Ordering::Acquire) != BSP_CONTEXT {
        return Err(SchedError::NotBackOnBsp);
    }

    let mut ids = [0u64; PREEMPT_TASKS];
    for (index, id) in ids.iter_mut().enumerate() {
        *id = PREEMPT_IDS[index].load(Ordering::Acquire);
    }
    let mut trace = [0u64; PREEMPT_TRACE_CAPACITY];
    for (index, entry) in trace.iter_mut().enumerate() {
        *entry = PREEMPT_TRACE[index].load(Ordering::Acquire);
    }

    let preemptions = PREEMPTIONS.load(Ordering::Acquire);
    if preemptions < PREEMPT_TARGET {
        return Err(SchedError::TooFewPreemptions);
    }
    if PREEMPT_TRACE_LEN.load(Ordering::Acquire) != PREEMPT_TRACE_CAPACITY
        || !preempt::is_rotation(&trace, &ids)
    {
        return Err(SchedError::NotRoundRobin);
    }
    let min_slices = preempt::min_slices(&trace, &ids);
    if min_slices < MIN_FAIR_SLICES {
        return Err(SchedError::TooFewSlices);
    }

    // Chaque tranche terminée par le timer l'a été pendant la charge de travail.
    for busy in &BUSY_PREEMPTIONS {
        if busy.load(Ordering::Acquire) < MIN_FAIR_SLICES {
            return Err(SchedError::TooFewSlices);
        }
    }

    let mut counters = [0u64; PREEMPT_TASKS];
    for (index, counter) in counters.iter_mut().enumerate() {
        *counter = COUNTERS[index].load(Ordering::Acquire);
        if *counter == 0 {
            return Err(SchedError::NoProgress);
        }
    }

    for &raw in &ids {
        if task::snapshot(task::lookup(raw)?)?.state != TaskState::Dead {
            return Err(SchedError::TaskNotDead);
        }
    }
    if !queue().is_empty() {
        return Err(SchedError::QueueNotEmpty);
    }
    if task::running_count() != 0 || RUNNING_MAX.load(Ordering::Acquire) != 1 {
        return Err(SchedError::RunningInvariant);
    }

    let exits = EXITS.load(Ordering::Acquire);
    if exits != PREEMPT_TASKS as u64
        || ARGS_OK.load(Ordering::Acquire) != PREEMPT_TASKS as u64
        || BSP_SWITCHES.load(Ordering::Acquire) != EXPECTED_BSP_SWITCHES
    {
        return Err(SchedError::CountMismatch);
    }

    let mut canaries_ok = 0;
    for region in &regions {
        if !stacks::canary_intact_at(*region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
    let bsp_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };
    if !task_stack::rsp_inside(bsp, bsp_rsp) {
        return Err(SchedError::BspRspOutsideKernelStack);
    }

    Ok(PreemptQualification {
        ids,
        trace,
        preemptions,
        task_ticks: TASK_TICKS.load(Ordering::Acquire),
        min_slices,
        counters,
        exits,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        canaries_ok,
    })
}

/// Index de la tâche préemptible courante, après contrôle de son argument
/// d'entrée. Échoue fail-closed : appelé au début de chaque entrée de tâche.
pub fn preemptible_task_index(arg: u64) -> usize {
    let current = CURRENT.load(Ordering::Acquire);
    let index = match preemptible_index_of(current) {
        Some(index) => index,
        None => fail(SchedError::UnknownCurrentTask),
    };
    if arg != ACTIVE_ARG_BASE.load(Ordering::Acquire) + index as u64 {
        fail(SchedError::ArgMismatch);
    }
    ARGS_OK.fetch_add(1, Ordering::AcqRel);
    index
}

/// Marque la tâche `index` comme occupée par sa charge de travail : les
/// préemptions subies pendant ce temps sont comptées à part.
pub fn set_busy(index: usize, busy: bool) {
    BUSY[index].store(busy, Ordering::Release);
}

/// Ajoute `amount` au compteur de progression de la tâche `index`.
pub fn add_progress(index: usize, amount: u64) {
    COUNTERS[index].fetch_add(amount, Ordering::AcqRel);
}

fn preemptible_index_of(task_id: u64) -> Option<usize> {
    PREEMPT_IDS
        .iter()
        .position(|id| id.load(Ordering::Acquire) == task_id)
}

fn reset_preemption() {
    PREEMPT_ACTIVE.store(false, Ordering::Release);
    ELYON_SCHED_STOP.store(false, Ordering::Release);
    PREEMPTIONS.store(0, Ordering::Release);
    TASK_TICKS.store(0, Ordering::Release);
    PREEMPT_TRACE_LEN.store(0, Ordering::Release);
    for entry in PREEMPT_TRACE
        .iter()
        .chain(COUNTERS.iter())
        .chain(BUSY_PREEMPTIONS.iter())
    {
        entry.store(0, Ordering::Release);
    }
    for busy in &BUSY {
        busy.store(false, Ordering::Release);
    }
    clock().reset();
}

fn clock() -> &'static mut SliceClock {
    // SAFETY: l'horloge de tranche n'est touchée que sur le BSP, IRQ masquées
    // (handler timer ou qualification), et aucune référence n'est conservée.
    unsafe { &mut *addr_of_mut!(CLOCK) }
}

fn record_slice(task_id: u64) -> Result<(), SchedError> {
    let slot = PREEMPT_TRACE_LEN.fetch_add(1, Ordering::AcqRel);
    PREEMPT_TRACE
        .get(slot)
        .ok_or(SchedError::TraceOverflow)?
        .store(task_id, Ordering::Release);
    Ok(())
}

#[unsafe(no_mangle)]
extern "C" fn elyon_preempt_task_entry(arg: u64) -> ! {
    let index = preemptible_task_index(arg);

    // Le frame initial porte IF=0 : la tâche ouvre elle-même les interruptions
    // pour que le timer puisse la préempter. Elle ne cède jamais volontairement.
    cpu::enable_interrupts();
    set_busy(index, true);
    while !ELYON_SCHED_STOP.load(Ordering::Acquire) {
        add_progress(index, 1);
        core::hint::spin_loop();
    }
    set_busy(index, false);

    exit_now()
}
