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

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use elyon_core::lock_order::LockClass;
use elyon_core::percpu_sched::{
    self, Expectation, Migration, Placed, PlacementError, TraceEntry,
    TraceError as PercpuTraceError,
};
use elyon_core::preempt::{self, Quantum, SliceClock, TickDecision};
use elyon_core::run_queue::{self, QueueError, RunQueue};
use elyon_core::task_stack::{self, StackRegion};
use elyon_core::timer_queue::{self, SleepRecord, TimerQueue, TimerQueueError};

use crate::arch::x86_64::cpu;
use crate::fpu::{self, FpuSetupError};
use crate::sync::{IrqGuard, IrqSpinLock};
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

/// Nombre maximal de CPU ayant leur propre état de scheduler.
pub const MAX_SCHED_CPUS: usize = 8;

// État de scheduler propre à chaque CPU logique : sa run queue, le RSP de son
// contexte de départ (hors tâche) et la tâche qu'il exécute. Chaque CPU ne
// touche que son propre slot, IRQ masquées.
static QUEUES: [IrqSpinLock<RunQueue<QUEUE_CAPACITY>>; MAX_SCHED_CPUS] =
    [const { IrqSpinLock::new(LockClass::RunQueue, RunQueue::new()) }; MAX_SCHED_CPUS];
static mut BOOT_RSP: [u64; MAX_SCHED_CPUS] = [0; MAX_SCHED_CPUS];

static CURRENT: [AtomicU64; MAX_SCHED_CPUS] =
    [const { AtomicU64::new(BSP_CONTEXT) }; MAX_SCHED_CPUS];
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
    Fpu(FpuSetupError),
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
    FpuAreaMissing,
    UnknownCpu,
    Placement(PlacementError),
    WrongCpu,
    ApTimeout,
    Trace,
    IdleState,
    TaskStillOnCpu,
    WakeDead,
    WakeRetriesExhausted,
    IpiFailed,
    EoiFailed,
    Timers(TimerQueueError),
    SleepTimeout,
    TimerQueueNotEmpty,
}

impl SchedError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task(error) => error.as_str(),
            Self::Stack(error) => error.as_str(),
            Self::Queue(error) => error.as_str(),
            Self::Timer(error) => error.as_str(),
            Self::Fpu(error) => error.as_str(),
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
            Self::FpuAreaMissing => "context has no FPU save area while FPU switching is on",
            Self::UnknownCpu => "current CPU is unknown or beyond the scheduler capacity",
            Self::Placement(error) => error.as_str(),
            Self::WrongCpu => "task runs on a CPU outside its affinity",
            Self::ApTimeout => "an AP did not finish its per-CPU run in time",
            Self::Trace => "per-CPU trace violates affinity, rotation or idle",
            Self::IdleState => "idle task not in its expected state",
            Self::Timers(error) => error.as_str(),
            Self::SleepTimeout => "a sleep phase did not complete in time",
            Self::TimerQueueNotEmpty => "deadlines left in the timer queue",
            Self::TaskStillOnCpu => "switching into a task still running on another CPU",
            Self::WakeDead => "wake of a terminated task",
            Self::WakeRetriesExhausted => "task never blocked while waiting to wake it",
            Self::IpiFailed => "resched IPI could not be sent",
            Self::EoiFailed => "EOI of the resched IPI failed",
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

impl From<FpuSetupError> for SchedError {
    fn from(error: FpuSetupError) -> Self {
        Self::Fpu(error)
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
        if fpu::enabled() {
            task::set_fpu_area(id, fpu::new_task_area()?)?;
        }
        let prepared = stacks::prepare(id, entry, ARG_BASE + index as u64, bsp, identity_limit)?;
        // SAFETY: l'emplacement appartient au slot statique de la tâche, qui n'a
        // encore jamais été exécutée ; seul le switch le relira.
        unsafe { write_volatile(task::saved_rsp_ptr(id)?, prepared.initial_rsp) };
        IDS[index].store(id.raw(), Ordering::Release);
        *region = prepared.region;
        queue().push_back(id.raw())?;
    }

    let first = run_queue::exit_next(&mut queue()).ok_or(SchedError::EmptyQueue)?;
    task::start(task::lookup(first)?, BSP_CPU)?;
    BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(BSP_CONTEXT, first)?;

    // Le BSP ne reprend la main qu'après la sortie de la dernière tâche.
    if CURRENT[this_cpu()].load(Ordering::Acquire) != BSP_CONTEXT {
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
    let bsp_rsp = unsafe { read_volatile(boot_rsp_slot(this_cpu())) };
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
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }

    task::yield_ready(task::lookup(current)?)?;
    let next = run_queue::yield_next(&mut queue(), current)?;
    YIELDS.fetch_add(1, Ordering::AcqRel);
    task::start(task::lookup(next)?, this_cpu() as u32)?;

    if next != current {
        TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
        transfer(current, next)?;
    }
    Ok(())
}

fn try_exit() -> Result<(), SchedError> {
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }

    task::terminate(task::lookup(current)?)?;
    EXITS.fetch_add(1, Ordering::AcqRel);

    // Le garde de file doit être relâché avant le switch : pas de temporaire
    // dans le scrutin du `match`, qui vivrait jusqu'à la fin du bloc.
    let next = run_queue::exit_next(&mut queue());
    match next {
        Some(next) => {
            task::start(task::lookup(next)?, this_cpu() as u32)?;
            TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
            transfer(current, next)
        }
        None => {
            // K004-07 : un CPU doté d'une idle task officielle bascule sur elle
            // quand sa file se vide ; sinon il revient à son contexte de départ.
            let idle = IDLE_IDS[this_cpu()].load(Ordering::Acquire);
            if idle != 0 {
                task::start(task::lookup(idle)?, this_cpu() as u32)?;
                TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
                transfer(current, idle)
            } else {
                BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
                transfer(current, BSP_CONTEXT)
            }
        }
    }
}

/// Passe le CPU du contexte `current` au contexte `next` (`BSP_CONTEXT` pour le
/// BSP). Les transitions d'état sont faites par l'appelant ; l'invariant « une
/// seule tâche RUNNING » est vérifié juste avant le switch.
fn transfer(current: u64, next: u64) -> Result<(), SchedError> {
    // K004-10 : aucun verrou emporté à travers le switch, IRQ masquées.
    crate::sync::assert_switch_allowed();
    let running = task::running_count_on(this_cpu() as u32);
    RUNNING_MAX.fetch_max(running, Ordering::AcqRel);
    let expected_running = if next == BSP_CONTEXT { 0 } else { 1 };
    if running != expected_running {
        return Err(SchedError::RunningInvariant);
    }

    let old_rsp = if current == BSP_CONTEXT {
        boot_rsp_slot(this_cpu())
    } else {
        task::saved_rsp_ptr(task::lookup(current)?)?
    };
    let new_rsp = if next == BSP_CONTEXT {
        // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
        unsafe { read_volatile(boot_rsp_slot(this_cpu())) }
    } else {
        // SAFETY: l'emplacement appartient au slot statique de la tâche ; il a
        // été écrit par qualify() ou par le dernier switch qui l'a quittée.
        unsafe { read_volatile(task::saved_rsp_ptr(task::lookup(next)?)?) }
    };

    if fpu::enabled() {
        let old_area = fpu_area_of(current)?;
        let new_area = fpu_area_of(next)?;
        // SAFETY: chaque contexte a sa propre zone (BSP ou tâche) ; on sauvegarde
        // l'état SIMD du sortant avant de charger celui de l'entrant (eager).
        unsafe {
            fpu::save(old_area);
            fpu::restore(new_area);
        }
    }

    // K004-10 : jamais entrer dans une tâche dont un autre CPU n'a pas fini de
    // sauvegarder le contexte. Ceux qui la remettent en file attendent que
    // `on_cpu` retombe ; ce contrôle fail-closed en est la garantie finale.
    if next != BSP_CONTEXT {
        let next_id = task::lookup(next)?;
        if task::is_on_cpu(next_id)? {
            return Err(SchedError::TaskStillOnCpu);
        }
        task::set_on_cpu(next_id, true)?;
    }
    crate::arch::x86_64::cpu_local::current()
        .switched_from
        .store(current, Ordering::Relaxed);
    CURRENT[this_cpu()].store(next, Ordering::Release);
    // SAFETY: `new_rsp` désigne le frame initial ou celui laissé par le dernier
    // switch de ce contexte, sur sa propre pile ; aucun verrou n'est tenu.
    unsafe { context::switch(old_rsp, new_rsp) };
    finish_switch();
    Ok(())
}

/// Exécuté par le contexte qui vient d'entrer sur ce CPU (au retour de son
/// switch, ou par le trampoline d'une tâche neuve) : le contexte quitté est
/// entièrement sauvegardé, il peut reprendre sur un autre CPU.
fn finish_switch() {
    let local = crate::arch::x86_64::cpu_local::current();
    let previous = local.switched_from.swap(BSP_CONTEXT, Ordering::Relaxed);
    if previous != BSP_CONTEXT {
        let cleared = task::lookup(previous).and_then(|id| task::set_on_cpu(id, false));
        if let Err(error) = cleared {
            fail(SchedError::Task(error));
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_finish_switch() {
    finish_switch();
}

/// Attentes observées d'une tâche encore en cours de sortie d'un autre CPU.
static ON_CPU_WAITS: AtomicU64 = AtomicU64::new(0);

/// Attend que `task_id` ait quitté tout CPU avant de la remettre en file.
fn wait_off_cpu(task_id: u64) -> Result<(), SchedError> {
    let id = task::lookup(task_id)?;
    if task::is_on_cpu(id)? {
        ON_CPU_WAITS.fetch_add(1, Ordering::Relaxed);
        while task::is_on_cpu(id)? {
            core::hint::spin_loop();
        }
    }
    Ok(())
}

fn fpu_area_of(context: u64) -> Result<u64, SchedError> {
    let area = if context == BSP_CONTEXT {
        fpu::cpu_area(this_cpu())
    } else {
        task::fpu_area(task::lookup(context)?)?
    };
    if area == 0 {
        return Err(SchedError::FpuAreaMissing);
    }
    Ok(area)
}

fn reset() {
    *queue() = RunQueue::new();
    CURRENT[this_cpu()].store(BSP_CONTEXT, Ordering::Release);
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

/// Run queue du CPU courant, verrouillée : le garde masque les IRQ et n'est
/// jamais conservé à travers un switch (vérifié par `transfer`).
fn queue() -> IrqGuard<'static, RunQueue<QUEUE_CAPACITY>> {
    queue_of(this_cpu())
}

fn queue_of(cpu: usize) -> IrqGuard<'static, RunQueue<QUEUE_CAPACITY>> {
    QUEUES[cpu].lock()
}

fn boot_rsp_slot(cpu: usize) -> *mut u64 {
    // SAFETY: `cpu < MAX_SCHED_CPUS` ; seul un pointeur brut est formé.
    unsafe { addr_of_mut!(BOOT_RSP).cast::<u64>().add(cpu) }
}

/// Index logique du CPU courant, retrouvé par son APIC ID. Fail-closed si le
/// CPU est inconnu ou au-delà de `MAX_SCHED_CPUS`.
pub fn this_cpu() -> usize {
    // Index lu via GS, réglé avant tout usage du scheduler sur ce CPU.
    match crate::arch::x86_64::cpu_local::index() {
        cpu if cpu < MAX_SCHED_CPUS => cpu,
        _ => fail(SchedError::UnknownCpu),
    }
}

/// L'index lu via GS doit désigner le slot dont l'APIC ID est celui de ce CPU :
/// sinon les verrous et les files seraient attribués à un autre CPU.
fn verify_cpu_index(expected: usize) -> Result<(), SchedError> {
    use crate::arch::x86_64::{apic, cpu_local, percpu};
    let by_apic = apic::current_id()
        .ok()
        .and_then(percpu::index_of_apic)
        .ok_or(SchedError::UnknownCpu)?;
    if by_apic != expected || cpu_local::index() != expected {
        return Err(SchedError::UnknownCpu);
    }
    Ok(())
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
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
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

static mut CLOCKS: [SliceClock; MAX_SCHED_CPUS] =
    [const { SliceClock::new(QUANTUM) }; MAX_SCHED_CPUS];

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
    // K004-09 : seul le timer du BSP est programmé ; il porte la file temporelle.
    if this_cpu() == 0 {
        expire_sleepers(timer::ticks());
    }
    if !PREEMPT_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
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
    let next = run_queue::yield_next(&mut queue(), current)?;
    task::start(task::lookup(next)?, this_cpu() as u32)?;
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
        if fpu::enabled() {
            task::set_fpu_area(id, fpu::new_task_area()?)?;
        }
        let prepared = stacks::prepare(id, entry, arg_base + index as u64, bsp, identity_limit)?;
        // SAFETY: l'emplacement appartient au slot statique de la tâche, qui n'a
        // encore jamais été exécutée ; seul le switch le relira.
        unsafe { write_volatile(task::saved_rsp_ptr(id)?, prepared.initial_rsp) };
        PREEMPT_IDS[index].store(id.raw(), Ordering::Release);
        *region = prepared.region;
        queue().push_back(id.raw())?;
    }

    let first = run_queue::exit_next(&mut queue()).ok_or(SchedError::EmptyQueue)?;
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
    if CURRENT[this_cpu()].load(Ordering::Acquire) != BSP_CONTEXT {
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
    let bsp_rsp = unsafe { read_volatile(boot_rsp_slot(this_cpu())) };
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
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
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

/// Préemptions subies par chaque tâche pendant qu'elle était marquée occupée,
/// lors du dernier `run_preemptive`.
pub fn busy_preemptions() -> [u64; PREEMPT_TASKS] {
    core::array::from_fn(|index| BUSY_PREEMPTIONS[index].load(Ordering::Acquire))
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
    // SAFETY: chaque CPU ne touche que son horloge de tranche, IRQ masquées
    // (handler timer ou qualification), et aucune référence n'est conservée.
    unsafe { &mut *addr_of_mut!(CLOCKS).cast::<SliceClock>().add(this_cpu()) }
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

// --- K004-07 : scheduler per-CPU et idle task officielle sur chaque CPU ----------
//
// Chaque CPU en ligne reçoit une idle task (affinité limitée à ce CPU) et deux
// tâches de travail placées par `percpu_sched::place`. Le BSP remplit toutes
// les files, publie `PERCPU_ARMED[cpu]` (Release) puis réveille chaque AP par
// IPI ; chaque CPU exécute alors ses tâches en Round-Robin coopératif depuis sa
// propre file. Une file vide bascule sur l'idle task du CPU : sur un AP elle
// tourne indéfiniment en `sti; hlt`, sur le BSP elle repasse READY et rend la
// main au boot. Les files ne sont jamais partagées : remplies par le BSP avant
// l'armement, puis touchées par leur seul CPU.

pub const PERCPU_TASKS_PER_CPU: usize = 2;
pub const PERCPU_ITERATIONS: u64 = 3;
pub const PERCPU_TRACE_CAPACITY: usize =
    MAX_SCHED_CPUS * (PERCPU_TASKS_PER_CPU * PERCPU_ITERATIONS as usize + 1);
const WORKER_ARG_BASE: u64 = 0xE1_0407_0000;
const IDLE_ARG_BASE: u64 = 0xE1_0407_8000;
/// Attente maximale de la fin des AP : 5000 × 1 ms.
const AP_WAIT_STEPS: u32 = 5_000;

static IDLE_IDS: [AtomicU64; MAX_SCHED_CPUS] = [const { AtomicU64::new(0) }; MAX_SCHED_CPUS];
static PERCPU_ARMED: [AtomicBool; MAX_SCHED_CPUS] =
    [const { AtomicBool::new(false) }; MAX_SCHED_CPUS];
static PERCPU_DONE: [AtomicBool; MAX_SCHED_CPUS] =
    [const { AtomicBool::new(false) }; MAX_SCHED_CPUS];
static AP_FPU_READY: [AtomicBool; MAX_SCHED_CPUS] =
    [const { AtomicBool::new(false) }; MAX_SCHED_CPUS];
static WORKER_IDS: [[AtomicU64; PERCPU_TASKS_PER_CPU]; MAX_SCHED_CPUS] =
    [const { [const { AtomicU64::new(0) }; PERCPU_TASKS_PER_CPU] }; MAX_SCHED_CPUS];
/// Passages : `(cpu + 1) << 56 | tâche`, 0 pour un emplacement vide.
static PERCPU_TRACE: [AtomicU64; PERCPU_TRACE_CAPACITY] =
    [const { AtomicU64::new(0) }; PERCPU_TRACE_CAPACITY];
static PERCPU_TRACE_LEN: AtomicUsize = AtomicUsize::new(0);

pub struct PercpuQualification {
    pub cpus: usize,
    pub idle_ids: [u64; MAX_SCHED_CPUS],
    pub worker_ids: [[u64; PERCPU_TASKS_PER_CPU]; MAX_SCHED_CPUS],
    pub trace: [TraceEntry; PERCPU_TRACE_CAPACITY],
    pub trace_len: usize,
    pub ap_fpu_ready: usize,
    pub idle_running_aps: usize,
    pub running_max: usize,
    pub canaries_ok: usize,
}

/// Point d'entrée du scheduler sur un AP, après sa mise en ligne. L'AP attend
/// en `sti; hlt` (il reste réveillable par IPI) que le BSP ait armé sa file,
/// configure son FPU, puis exécute sa file.
pub fn ap_enter(cpu: usize) -> ! {
    if cpu >= MAX_SCHED_CPUS {
        cpu::idle_forever();
    }
    if let Err(error) = verify_cpu_index(cpu) {
        fail(error);
    }
    while !PERCPU_ARMED[cpu].load(Ordering::Acquire) {
        // SAFETY: attente passive ; une IRQ (IPI de réveil) fait sortir du hlt.
        unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
    }
    cpu::disable_interrupts();

    if let Err(error) = fpu::init_secondary() {
        kprintln!(
            "[percpu-sched] FAILED cpu={} reason={}",
            cpu,
            error.as_str()
        );
        cpu::halt_forever();
    }
    AP_FPU_READY[cpu].store(true, Ordering::Release);

    if let Err(error) = run_own_queue(cpu) {
        fail(error);
    }
    // La dernière tâche de l'AP bascule sur son idle task, qui ne rend jamais
    // la main au contexte de départ.
    fail(SchedError::IdleState)
}

fn run_own_queue(cpu: usize) -> Result<(), SchedError> {
    let first = run_queue::exit_next(&mut queue_of(cpu)).ok_or(SchedError::EmptyQueue)?;
    task::start(task::lookup(first)?, cpu as u32)?;
    BSP_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(BSP_CONTEXT, first)
}

pub fn qualify_percpu(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<PercpuQualification, SchedError> {
    use crate::arch::x86_64::{apic, percpu as cpus};

    cpu::disable_interrupts();
    if this_cpu() != 0 {
        return Err(SchedError::UnknownCpu);
    }
    verify_cpu_index(0)?;
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };

    let count = cpus::cpu_count().min(MAX_SCHED_CPUS);
    let mut online_mask = 0u64;
    for index in 0..count {
        if cpus::is_online(index) {
            online_mask |= 1 << index;
        }
    }

    reset();
    for index in 0..MAX_SCHED_CPUS {
        *queue_of(index) = RunQueue::new();
        IDLE_IDS[index].store(0, Ordering::Release);
        PERCPU_DONE[index].store(false, Ordering::Release);
        AP_FPU_READY[index].store(false, Ordering::Release);
    }
    PERCPU_TRACE_LEN.store(0, Ordering::Release);
    for entry in &PERCPU_TRACE {
        entry.store(0, Ordering::Release);
    }

    let idle_entry = elyon_percpu_idle_entry as *const () as usize as u64;
    let worker_entry = elyon_percpu_worker_entry as *const () as usize as u64;
    let mut regions =
        [StackRegion { start: 0, end: 0 }; MAX_SCHED_CPUS * (PERCPU_TASKS_PER_CPU + 1)];
    let mut region_count = 0;

    for cpu_index in (0..count).filter(|index| online_mask & (1 << index) != 0) {
        if cpu_index != 0 {
            fpu::prepare_cpu_area(cpu_index)?;
        }
        let affinity = 1u64 << cpu_index;

        let idle = spawn_on(
            affinity,
            idle_entry,
            IDLE_ARG_BASE + cpu_index as u64,
            bsp,
            identity_limit,
        )?;
        IDLE_IDS[cpu_index].store(idle.0, Ordering::Release);
        regions[region_count] = idle.1;
        region_count += 1;

        for (slot, worker_id) in WORKER_IDS[cpu_index].iter().enumerate() {
            let placed =
                percpu_sched::place(affinity, online_mask).map_err(SchedError::Placement)?;
            if placed != cpu_index {
                return Err(SchedError::WrongCpu);
            }
            let arg = WORKER_ARG_BASE + (cpu_index as u64) * 16 + slot as u64;
            let worker = spawn_on(affinity, worker_entry, arg, bsp, identity_limit)?;
            worker_id.store(worker.0, Ordering::Release);
            regions[region_count] = worker.1;
            region_count += 1;
            queue_of(placed).push_back(worker.0)?;
        }
    }

    // Armement : les files des AP sont complètes avant d'être publiées.
    for cpu_index in (1..count).filter(|index| online_mask & (1 << index) != 0) {
        PERCPU_ARMED[cpu_index].store(true, Ordering::Release);
        let apic_id = cpus::snapshot(cpu_index)
            .ok_or(SchedError::UnknownCpu)?
            .apic_id;
        apic::send_fixed(apic_id, crate::arch::x86_64::smp::STABILITY_VECTOR)
            .map_err(|_| SchedError::ApTimeout)?;
    }

    // Le BSP exécute sa propre file ; son idle task lui rend la main.
    run_own_queue(0)?;

    let mut waited = 0;
    while (0..count)
        .filter(|index| online_mask & (1 << index) != 0)
        .any(|index| !PERCPU_DONE[index].load(Ordering::Acquire))
    {
        if waited >= AP_WAIT_STEPS {
            return Err(SchedError::ApTimeout);
        }
        timer::pit_delay_us(1_000)?;
        waited += 1;
    }

    let cpus_online = online_mask.count_ones() as usize;
    let mut idle_ids = [0u64; MAX_SCHED_CPUS];
    let mut worker_ids = [[0u64; PERCPU_TASKS_PER_CPU]; MAX_SCHED_CPUS];
    for index in 0..cpus_online {
        idle_ids[index] = IDLE_IDS[index].load(Ordering::Acquire);
        for slot in 0..PERCPU_TASKS_PER_CPU {
            worker_ids[index][slot] = WORKER_IDS[index][slot].load(Ordering::Acquire);
        }
    }

    let trace_len = PERCPU_TRACE_LEN
        .load(Ordering::Acquire)
        .min(PERCPU_TRACE_CAPACITY);
    let mut trace = [TraceEntry { cpu: 0, task: 0 }; PERCPU_TRACE_CAPACITY];
    for (index, entry) in trace.iter_mut().enumerate().take(trace_len) {
        let raw = PERCPU_TRACE[index].load(Ordering::Acquire);
        *entry = TraceEntry {
            cpu: ((raw >> 56) as u32).wrapping_sub(1),
            task: raw & ((1 << 56) - 1),
        };
    }

    let task_lists: [&[u64]; MAX_SCHED_CPUS] = core::array::from_fn(|index| &worker_ids[index][..]);
    percpu_sched::check_trace(
        &trace[..trace_len],
        &Expectation {
            tasks: &task_lists[..cpus_online],
            idle: &idle_ids[..cpus_online],
            iterations: PERCPU_ITERATIONS,
        },
    )
    .map_err(|error: PercpuTraceError| {
        kprintln!("[percpu-sched] trace_error={:?}", error);
        SchedError::Trace
    })?;

    for workers in worker_ids.iter().take(cpus_online) {
        for &worker in workers {
            if task::snapshot(task::lookup(worker)?)?.state != TaskState::Dead {
                return Err(SchedError::TaskNotDead);
            }
        }
    }

    // Idle du BSP : READY, parquée. Idle des AP : RUNNING sur leur CPU.
    let bsp_idle = task::snapshot(task::lookup(idle_ids[0])?)?;
    if bsp_idle.state != TaskState::Ready {
        return Err(SchedError::IdleState);
    }
    let mut idle_running_aps = 0;
    for (index, &idle) in idle_ids.iter().enumerate().take(cpus_online).skip(1) {
        let snapshot = task::snapshot(task::lookup(idle)?)?;
        if snapshot.state != TaskState::Running || snapshot.current_cpu != index as u32 {
            return Err(SchedError::IdleState);
        }
        idle_running_aps += 1;
    }
    if task::running_count() != idle_running_aps {
        return Err(SchedError::RunningInvariant);
    }

    let ap_fpu_ready = (1..cpus_online)
        .filter(|&index| AP_FPU_READY[index].load(Ordering::Acquire))
        .count();
    if ap_fpu_ready != cpus_online - 1 {
        return Err(SchedError::Fpu(FpuSetupError::NotEnabled));
    }

    let mut canaries_ok = 0;
    for region in &regions[..region_count] {
        if !stacks::canary_intact_at(*region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    Ok(PercpuQualification {
        cpus: cpus_online,
        idle_ids,
        worker_ids,
        trace,
        trace_len,
        ap_fpu_ready,
        idle_running_aps,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        canaries_ok,
    })
}

fn spawn_on(
    affinity: u64,
    entry: u64,
    arg: u64,
    bsp: StackRegion,
    identity_limit: u64,
) -> Result<(u64, StackRegion), SchedError> {
    let id = task::create_kernel(affinity)?;
    if fpu::enabled() {
        task::set_fpu_area(id, fpu::new_task_area()?)?;
    }
    let prepared = stacks::prepare(id, entry, arg, bsp, identity_limit)?;
    // SAFETY: l'emplacement appartient au slot statique de la tâche, qui n'a
    // encore jamais été exécutée ; seul le switch le relira.
    unsafe { write_volatile(task::saved_rsp_ptr(id)?, prepared.initial_rsp) };
    Ok((id.raw(), prepared.region))
}

fn record_percpu(cpu: usize, task_id: u64) {
    let slot = PERCPU_TRACE_LEN.fetch_add(1, Ordering::AcqRel);
    match PERCPU_TRACE.get(slot) {
        Some(entry) => entry.store(((cpu as u64 + 1) << 56) | task_id, Ordering::Release),
        None => fail(SchedError::TraceOverflow),
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_percpu_worker_entry(arg: u64) -> ! {
    let cpu = this_cpu();
    let current = CURRENT[cpu].load(Ordering::Acquire);
    if arg.wrapping_sub(WORKER_ARG_BASE) / 16 != cpu as u64 {
        fail(SchedError::WrongCpu);
    }
    for iteration in 0..PERCPU_ITERATIONS {
        record_percpu(cpu, current);
        if iteration + 1 < PERCPU_ITERATIONS {
            yield_now();
        }
    }
    exit_now()
}

#[unsafe(no_mangle)]
extern "C" fn elyon_percpu_idle_entry(arg: u64) -> ! {
    let cpu = this_cpu();
    let current = CURRENT[cpu].load(Ordering::Acquire);
    if arg != IDLE_ARG_BASE + cpu as u64 {
        fail(SchedError::ArgMismatch);
    }
    record_percpu(cpu, current);
    PERCPU_DONE[cpu].store(true, Ordering::Release);

    if cpu == 0 {
        // BSP : l'idle task se parque READY et rend la main au boot, autant de
        // fois que le BSP vide sa file (K004-07 puis K004-08).
        loop {
            cpu::disable_interrupts();
            let result = task::lookup(current)
                .and_then(task::yield_ready)
                .map_err(SchedError::Task)
                .and_then(|_| transfer(current, BSP_CONTEXT));
            if let Err(error) = result {
                fail(error);
            }
        }
    }

    // AP : idle officielle, réveillable par IRQ, jamais terminée.
    cpu::idle_forever()
}

// --- K004-08 : IPI de rescheduling -------------------------------------------------

/// Vecteur de l'IPI qui signale à un CPU qu'une tâche a été placée dans sa file.
pub const RESCHED_VECTOR: u8 = 0xF2;
/// Marge laissée à un IPI encore en vol avant de relever les compteurs.
const IPI_SETTLE_US: u32 = 2_000;

/// Le LAPIC ne garde qu'une interruption en attente par vecteur : un IPI de
/// rescheduling envoyé pendant qu'un autre attend est absorbé par lui, et un
/// seul passage dans le handler vide toute la file. Les reçus ne peuvent donc
/// pas être exigés égaux aux envoyés ; la preuve qu'aucun réveil n'est perdu
/// est que toutes les tâches terminent. Ici : nombre d'envois exact, et chaque
/// réception correspond à un envoi.
fn ipi_counts_consistent(sent: u64, received: u64, expected_sent: u64) -> bool {
    sent == expected_sent && received >= 1 && received <= sent
}

static RESCHED_SENT: AtomicU64 = AtomicU64::new(0);
static RESCHED_RECEIVED: AtomicU64 = AtomicU64::new(0);

/// Handler de l'IPI de rescheduling, IRQ masquées : EOI d'abord, puis le CPU
/// reprend la main sur sa file.
pub fn on_resched_ipi() {
    RESCHED_RECEIVED.fetch_add(1, Ordering::AcqRel);
    if !crate::arch::x86_64::apic::eoi() {
        fail(SchedError::EoiFailed);
    }
    if let Err(error) = leave_idle_if_work() {
        fail(error);
    }
}

/// Si le CPU exécute son idle task et que sa file contient une tâche, l'idle
/// repasse READY et la tâche prend le CPU, depuis l'IRQ. L'idle reprendra plus
/// tard ici même, quand la file sera de nouveau vide, puis sortira par `iretq`.
fn leave_idle_if_work() -> Result<(), SchedError> {
    let cpu = this_cpu();
    let idle = IDLE_IDS[cpu].load(Ordering::Acquire);
    let current = CURRENT[cpu].load(Ordering::Acquire);
    if idle == 0 || current != idle {
        return Ok(());
    }
    let next = run_queue::exit_next(&mut queue());
    let Some(next) = next else {
        return Ok(());
    };
    task::yield_ready(task::lookup(idle)?)?;
    task::start(task::lookup(next)?, cpu as u32)?;
    TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
    transfer(idle, next)
}

fn online_mask() -> u64 {
    use crate::arch::x86_64::percpu as cpus;
    let count = cpus::cpu_count().min(MAX_SCHED_CPUS);
    (0..count)
        .filter(|&index| cpus::is_online(index))
        .fold(0u64, |mask, index| mask | (1 << index))
}

fn send_resched(target: usize) -> Result<(), SchedError> {
    if !percpu_sched::should_send_ipi(target, this_cpu()) {
        return Ok(());
    }
    let apic_id = crate::arch::x86_64::percpu::snapshot(target)
        .ok_or(SchedError::UnknownCpu)?
        .apic_id;
    crate::arch::x86_64::apic::send_fixed(apic_id, RESCHED_VECTOR)
        .map_err(|_| SchedError::IpiFailed)?;
    RESCHED_SENT.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

/// Exécute `body` IRQ locales masquées, puis restaure IF (irq-save).
fn irq_save<T>(body: impl FnOnce() -> T) -> T {
    let interrupts = cpu::interrupts_enabled();
    cpu::disable_interrupts();
    let result = body();
    if interrupts {
        cpu::enable_interrupts();
    }
    result
}

/// Place une tâche READY dans la file du CPU choisi par son affinité et, si
/// `notify`, prévient ce CPU par IPI de rescheduling. Retourne le CPU choisi.
pub fn enqueue(task_id: u64, notify: bool) -> Result<usize, SchedError> {
    irq_save(|| {
        let snapshot = task::snapshot(task::lookup(task_id)?)?;
        if snapshot.state != TaskState::Ready {
            return Err(SchedError::Task(TaskError::InvalidTransition));
        }
        let cpu = percpu_sched::place(snapshot.affinity_mask, online_mask())
            .map_err(SchedError::Placement)?;
        queue_of(cpu).push_back(task_id)?;
        if notify {
            send_resched(cpu)?;
        }
        Ok(cpu)
    })
}

/// Réveille une tâche BLOCKED ou SLEEPING : la transition vers READY est
/// atomique ; seul le gagnant la remet en file et prévient son CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    /// La tâche est remise dans la file de ce CPU.
    Woken(usize),
    /// Elle n'était pas endormie : déjà réveillée par un autre CPU ou par son
    /// échéance, ou pas encore bloquée. Entre réveils concurrents, un seul
    /// obtient `Woken`.
    NotAsleep,
}

/// Réveille une tâche BLOCKED ou SLEEPING (K004-10). Réveiller une tâche
/// terminée est une erreur, jamais un simple `NotAsleep`.
pub fn wake(task_id: u64) -> Result<WakeOutcome, SchedError> {
    irq_save(|| {
        // Un réveil explicite annule d'abord l'échéance en attente : le tick ne
        // pourra plus réveiller la tâche une seconde fois (K004-09). Une erreur
        // ensuite est fatale pour l'appelant.
        let cancelled = timers().cancel(task_id);
        let outcome = wake_task(task_id)?;
        if cancelled && matches!(outcome, WakeOutcome::Woken(_)) {
            log_wake(task_id, timer::ticks(), true);
            EXPLICIT_WAKES.fetch_add(1, Ordering::AcqRel);
        }
        Ok(outcome)
    })
}

/// Remet en file une tâche BLOCKED ou SLEEPING et prévient son CPU, IRQ
/// masquées. Transactionnel : l'état ne change que si la mise en file est
/// assurée, et jamais avant que la tâche ait quitté son ancien CPU.
fn wake_task(task_id: u64) -> Result<WakeOutcome, SchedError> {
    let id = task::lookup(task_id)?;
    let snapshot = task::snapshot(id)?;
    match snapshot.state {
        TaskState::Blocked | TaskState::Sleeping => {}
        TaskState::Dead => return Err(SchedError::WakeDead),
        _ => return Ok(WakeOutcome::NotAsleep),
    }
    wait_off_cpu(task_id)?;
    let cpu = percpu_sched::place(snapshot.affinity_mask, online_mask())
        .map_err(SchedError::Placement)?;
    {
        let mut queue = queue_of(cpu);
        if queue.len() == RunQueue::<QUEUE_CAPACITY>::capacity() {
            return Err(SchedError::Queue(QueueError::Full));
        }
        match task::wake(id) {
            Ok(()) => {}
            Err(TaskError::InvalidTransition) => return Ok(WakeOutcome::NotAsleep),
            Err(error) => return Err(SchedError::Task(error)),
        }
        // Place vérifiée, et une tâche endormie n'est dans aucune file.
        queue.push_back(task_id)?;
    }
    send_resched(cpu)?;
    Ok(WakeOutcome::Woken(cpu))
}

/// Change l'affinité d'une tâche READY en file et la migre si son CPU actuel
/// sort du nouveau masque. Retourne la décision prise.
pub fn set_affinity(task_id: u64, affinity: u64) -> Result<Migration, SchedError> {
    irq_save(|| {
        let mask = online_mask();
        let current = (0..MAX_SCHED_CPUS).find(|&cpu| queue_of(cpu).contains(task_id));
        let decision =
            percpu_sched::migrate(current, affinity, mask).map_err(SchedError::Placement)?;
        task::set_affinity(task::lookup(task_id)?, affinity)?;
        match decision {
            Migration::Stay(_) => {}
            Migration::Move { from, to } => {
                // Jamais deux verrous de file à la fois : retrait puis ajout. Si
                // `from` l'a déjà prise entre-temps, elle tourne là-bas et suivra
                // son nouveau masque au prochain réveil.
                if !queue_of(from).remove(task_id) {
                    return Ok(Migration::Stay(from));
                }
                // Une tâche qui vient de céder son CPU peut être encore en train
                // d'y sauvegarder son contexte.
                wait_off_cpu(task_id)?;
                queue_of(to).push_back(task_id)?;
                send_resched(to)?;
            }
            // Dans aucune file : RUNNING, BLOCKED ou SLEEPING. Le nouveau masque
            // s'applique à son prochain placement ; la mettre en file ici la
            // ferait tourner deux fois.
            Migration::Place(_) => {}
        }
        Ok(decision)
    })
}

/// Bloque la tâche courante. Le passage à BLOCKED et le choix de la suivante se
/// font sous le verrou de la file du CPU : un réveil concurrent, qui doit
/// prendre ce verrou pour remettre la tâche en file, ne peut pas s'intercaler.
pub fn block_now() {
    cpu::disable_interrupts();
    if let Err(error) = try_block() {
        fail(error);
    }
    // Reprise après un `wake()` : la tâche est RUNNING sur son CPU.
    cpu::enable_interrupts();
}

fn try_block() -> Result<(), SchedError> {
    let cpu = this_cpu();
    let current = CURRENT[cpu].load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }
    let next = {
        let mut guard = queue();
        task::block(task::lookup(current)?)?;
        run_queue::exit_next(&mut guard)
    };
    BLOCKS.fetch_add(1, Ordering::AcqRel);
    switch_away(cpu, current, next)
}

/// Quitte le CPU pour la tâche suivante de sa file, ou pour son idle task si la
/// file est vide ; la tâche courante a déjà quitté l'état RUNNING.
fn switch_away(cpu: usize, current: u64, next: Option<u64>) -> Result<(), SchedError> {
    match next {
        Some(next) => {
            task::start(task::lookup(next)?, cpu as u32)?;
            TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
            transfer(current, next)
        }
        None => {
            let idle = IDLE_IDS[cpu].load(Ordering::Acquire);
            if idle == 0 {
                return Err(SchedError::IdleState);
            }
            task::start(task::lookup(idle)?, cpu as u32)?;
            IDLE_ENTRIES[cpu].fetch_add(1, Ordering::AcqRel);
            TASK_SWITCHES.fetch_add(1, Ordering::AcqRel);
            transfer(current, idle)
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_resched_ipi_dispatch() {
    on_resched_ipi();
}

// --- K004-08 : qualification du rescheduling à distance ---------------------------
//
// Les AP sont dans leur idle task (fin de K004-07). Trois scénarios, sur des CPU
// qui tournent déjà :
// - migration : une tâche READY placée sans IPI sur l'AP 1 change d'affinité
//   vers le CPU 0 ; elle est retirée de la file de l'AP 1 et le BSP l'exécute ;
// - création distante : une tâche placée sur l'AP 1 avec IPI ; l'idle de l'AP 1
//   lui cède le CPU depuis le handler d'IPI ;
// - blocage et réveil : une tâche placée sur un AP se bloque ; le BSP attend
//   qu'elle soit BLOCKED puis la réveille, et elle reprend sur son CPU.

const REMOTE_ARG_BASE: u64 = 0xE1_0408_0000;
const ROLE_SPAWN: u64 = 0;
const ROLE_BLOCK: u64 = 1;
const ROLE_MIGRATE: u64 = 2;
const REMOTE_TASKS: u64 = 3;
const REMOTE_TRACE_CAPACITY: usize = 8;
/// Attente maximale d'un changement d'état distant : 5000 × 1 ms.
const REMOTE_WAIT_STEPS: u32 = 5_000;

static BLOCKS: AtomicU64 = AtomicU64::new(0);
static REMOTE_DONE: AtomicU64 = AtomicU64::new(0);
static REMOTE_TRACE: [AtomicU64; REMOTE_TRACE_CAPACITY] =
    [const { AtomicU64::new(0) }; REMOTE_TRACE_CAPACITY];
static REMOTE_TRACE_LEN: AtomicUsize = AtomicUsize::new(0);

pub struct RemoteQualification {
    pub cpus: usize,
    pub spawned: u64,
    pub spawn_cpu: usize,
    pub blocked: u64,
    pub block_cpu: usize,
    pub migrated: u64,
    pub migrate_from: usize,
    pub migrate_to: usize,
    pub ipi_sent: u64,
    pub ipi_received: u64,
    pub blocks: u64,
    pub contention: u64,
    pub idle_running_aps: usize,
    pub running_max: usize,
    pub canaries_ok: usize,
}

/// `Ok(None)` sur une machine sans AP : rien à qualifier à distance.
pub fn qualify_remote(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<Option<RemoteQualification>, SchedError> {
    cpu::disable_interrupts();
    if this_cpu() != 0 {
        return Err(SchedError::UnknownCpu);
    }
    let mask = online_mask();
    let cpus = mask.count_ones() as usize;
    if cpus < 2 {
        return Ok(None);
    }
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let spawn_cpu = 1usize;
    let block_cpu = if cpus >= 3 { 2 } else { 1 };
    let migrate_from = 1usize;
    let migrate_to = 0usize;

    BLOCKS.store(0, Ordering::Release);
    REMOTE_DONE.store(0, Ordering::Release);
    REMOTE_TRACE_LEN.store(0, Ordering::Release);
    RESCHED_SENT.store(0, Ordering::Release);
    RESCHED_RECEIVED.store(0, Ordering::Release);
    RUNNING_MAX.store(0, Ordering::Release);
    let entry = elyon_remote_task_entry as *const () as usize as u64;

    // Migration : placée sans IPI sur l'AP 1, qui reste donc en idle, puis
    // déplacée vers le CPU 0 avant d'avoir jamais tourné.
    let migrated = spawn_on(
        1 << migrate_from,
        entry,
        REMOTE_ARG_BASE + ROLE_MIGRATE,
        bsp,
        identity_limit,
    )?;
    if enqueue(migrated.0, false)? != migrate_from {
        return Err(SchedError::WrongCpu);
    }
    match set_affinity(migrated.0, 1 << migrate_to)? {
        Migration::Move { from, to } if from == migrate_from && to == migrate_to => {}
        _ => return Err(SchedError::WrongCpu),
    }

    // Création distante sur un AP déjà en idle.
    let spawned = spawn_on(
        1 << spawn_cpu,
        entry,
        REMOTE_ARG_BASE + ROLE_SPAWN,
        bsp,
        identity_limit,
    )?;
    if enqueue(spawned.0, true)? != spawn_cpu {
        return Err(SchedError::WrongCpu);
    }

    // Blocage sur un AP puis réveil depuis le BSP.
    let blocked = spawn_on(
        1 << block_cpu,
        entry,
        REMOTE_ARG_BASE + ROLE_BLOCK,
        bsp,
        identity_limit,
    )?;
    if enqueue(blocked.0, true)? != block_cpu {
        return Err(SchedError::WrongCpu);
    }
    wait_until(|| {
        task::lookup(blocked.0)
            .and_then(task::snapshot)
            .is_ok_and(|snapshot| snapshot.state == TaskState::Blocked)
    })?;
    if wake(blocked.0)? != WakeOutcome::Woken(block_cpu) {
        return Err(SchedError::WrongCpu);
    }

    // Le BSP exécute la tâche migrée ; son idle task lui rend la main.
    run_own_queue(0)?;

    wait_until(|| REMOTE_DONE.load(Ordering::Acquire) == REMOTE_TASKS)?;
    // Les AP reviennent dans leur idle task juste après la sortie de leur tâche.
    let idles: [u64; MAX_SCHED_CPUS] =
        core::array::from_fn(|index| IDLE_IDS[index].load(Ordering::Acquire));
    wait_until(|| (1..cpus).all(|index| CURRENT[index].load(Ordering::Acquire) == idles[index]))?;
    // Marge pour un IPI encore en vol ; les IPI fusionnés ne seront jamais reçus.
    timer::pit_delay_us(IPI_SETTLE_US)?;

    let trace_len = REMOTE_TRACE_LEN
        .load(Ordering::Acquire)
        .min(REMOTE_TRACE_CAPACITY);
    let mut trace = [TraceEntry { cpu: 0, task: 0 }; REMOTE_TRACE_CAPACITY];
    for (index, slot) in trace.iter_mut().enumerate().take(trace_len) {
        let raw = REMOTE_TRACE[index].load(Ordering::Acquire);
        *slot = TraceEntry {
            cpu: ((raw >> 56) as u32).wrapping_sub(1),
            task: raw & ((1 << 56) - 1),
        };
    }
    percpu_sched::check_placement(
        &trace[..trace_len],
        &[
            Placed {
                task: spawned.0,
                cpu: spawn_cpu as u32,
                runs: 1,
            },
            Placed {
                task: blocked.0,
                cpu: block_cpu as u32,
                runs: 2,
            },
            Placed {
                task: migrated.0,
                cpu: migrate_to as u32,
                runs: 1,
            },
        ],
    )
    .map_err(|error: PercpuTraceError| {
        kprintln!("[sched-ipi] trace_error={:?}", error);
        SchedError::Trace
    })?;

    for id in [spawned.0, blocked.0, migrated.0] {
        if task::snapshot(task::lookup(id)?)?.state != TaskState::Dead {
            return Err(SchedError::TaskNotDead);
        }
    }
    let mut idle_running_aps = 0;
    for (index, &idle) in idles.iter().enumerate().take(cpus).skip(1) {
        let snapshot = task::snapshot(task::lookup(idle)?)?;
        if snapshot.state != TaskState::Running || snapshot.current_cpu != index as u32 {
            return Err(SchedError::IdleState);
        }
        idle_running_aps += 1;
    }

    let ipi_sent = RESCHED_SENT.load(Ordering::Acquire);
    let ipi_received = RESCHED_RECEIVED.load(Ordering::Acquire);
    // Création distante, première mise en file du bloqué, réveil : trois IPI ;
    // la migration vers le CPU 0 n'en demande pas.
    if !ipi_counts_consistent(ipi_sent, ipi_received, 3) || BLOCKS.load(Ordering::Acquire) != 1 {
        return Err(SchedError::CountMismatch);
    }

    let mut canaries_ok = 0;
    for region in [spawned.1, blocked.1, migrated.1] {
        if !stacks::canary_intact_at(region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    Ok(Some(RemoteQualification {
        cpus,
        spawned: spawned.0,
        spawn_cpu,
        blocked: blocked.0,
        block_cpu,
        migrated: migrated.0,
        migrate_from,
        migrate_to,
        ipi_sent,
        ipi_received,
        blocks: BLOCKS.load(Ordering::Acquire),
        contention: crate::sync::total_contention(),
        idle_running_aps,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        canaries_ok,
    }))
}

fn wait_until(mut condition: impl FnMut() -> bool) -> Result<(), SchedError> {
    for _ in 0..REMOTE_WAIT_STEPS {
        if condition() {
            return Ok(());
        }
        timer::pit_delay_us(1_000)?;
    }
    if condition() {
        Ok(())
    } else {
        Err(SchedError::ApTimeout)
    }
}

fn record_remote(cpu: usize, task_id: u64) {
    let slot = REMOTE_TRACE_LEN.fetch_add(1, Ordering::AcqRel);
    match REMOTE_TRACE.get(slot) {
        Some(entry) => entry.store(((cpu as u64 + 1) << 56) | task_id, Ordering::Release),
        None => fail(SchedError::TraceOverflow),
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_remote_task_entry(arg: u64) -> ! {
    let cpu = this_cpu();
    let current = CURRENT[cpu].load(Ordering::Acquire);
    let role = arg.wrapping_sub(REMOTE_ARG_BASE);
    if role >= REMOTE_TASKS {
        fail(SchedError::ArgMismatch);
    }
    record_remote(cpu, current);
    if role == ROLE_BLOCK {
        block_now();
        record_remote(this_cpu(), current);
    }
    REMOTE_DONE.fetch_add(1, Ordering::AcqRel);
    exit_now()
}

// --- K004-09 : sommeil et file temporelle ------------------------------------------
//
// Une file temporelle globale, protégée par un verrou, est vidée par le tick du
// BSP (seul timer programmé). Une échéance atteinte réveille la tâche par le même
// chemin que `wake()` : remise dans la file de son CPU, puis IPI de rescheduling.
// Ordre des verrous : file du CPU puis file temporelle (sleep) ; le tick relâche
// la file temporelle avant de prendre une file de CPU.

const TIMER_CAPACITY: usize = 16;
static TIMERS: IrqSpinLock<TimerQueue<TIMER_CAPACITY>> =
    IrqSpinLock::new(LockClass::TimerQueue, TimerQueue::new());

static SLEEPS: AtomicU64 = AtomicU64::new(0);
static TIMER_WAKES: AtomicU64 = AtomicU64::new(0);
static EXPLICIT_WAKES: AtomicU64 = AtomicU64::new(0);
static STALE_EXPIRIES: AtomicU64 = AtomicU64::new(0);
/// Passages d'un CPU à son idle task parce que sa tâche s'est bloquée ou endormie.
static IDLE_ENTRIES: [AtomicU64; MAX_SCHED_CPUS] = [const { AtomicU64::new(0) }; MAX_SCHED_CPUS];

/// File temporelle verrouillée, IRQ masquées, jamais à travers un switch.
fn timers() -> IrqGuard<'static, TimerQueue<TIMER_CAPACITY>> {
    TIMERS.lock()
}

// Journal des sommeils, pour la qualification : un enregistrement par sommeil.
const SLEEP_LOG_CAPACITY: usize = 8;
static SLEEP_LOG_LEN: AtomicUsize = AtomicUsize::new(0);
static SLEEP_TASK: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_START: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_DEADLINE: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_WOKE_AT: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_RESUMED_AT: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_WAKES: [AtomicU64; SLEEP_LOG_CAPACITY] =
    [const { AtomicU64::new(0) }; SLEEP_LOG_CAPACITY];
static SLEEP_EXPLICIT: [AtomicBool; SLEEP_LOG_CAPACITY] =
    [const { AtomicBool::new(false) }; SLEEP_LOG_CAPACITY];

/// Le journal n'enregistre que pendant la qualification K004-09 : hors de cette
/// fenêtre, `sleep_ms()` ne dépend pas de sa capacité.
static SLEEP_LOG_ON: AtomicBool = AtomicBool::new(false);

fn log_sleep(task_id: u64, start: u64, deadline: u64) -> Result<(), SchedError> {
    if !SLEEP_LOG_ON.load(Ordering::Acquire) {
        return Ok(());
    }
    let index = SLEEP_LOG_LEN.fetch_add(1, Ordering::AcqRel);
    if index >= SLEEP_LOG_CAPACITY {
        return Err(SchedError::TraceOverflow);
    }
    SLEEP_START[index].store(start, Ordering::Release);
    SLEEP_DEADLINE[index].store(deadline, Ordering::Release);
    SLEEP_WAKES[index].store(0, Ordering::Release);
    SLEEP_EXPLICIT[index].store(false, Ordering::Release);
    SLEEP_TASK[index].store(task_id, Ordering::Release);
    Ok(())
}

/// Dernier sommeil enregistré de `task_id`.
fn sleep_record_of(task_id: u64) -> Option<usize> {
    let len = SLEEP_LOG_LEN
        .load(Ordering::Acquire)
        .min(SLEEP_LOG_CAPACITY);
    (0..len)
        .rev()
        .find(|&index| SLEEP_TASK[index].load(Ordering::Acquire) == task_id)
}

fn log_wake(task_id: u64, now: u64, explicit: bool) {
    match sleep_record_of(task_id) {
        Some(index) => {
            SLEEP_WOKE_AT[index].store(now, Ordering::Release);
            SLEEP_EXPLICIT[index].store(explicit, Ordering::Release);
            SLEEP_WAKES[index].fetch_add(1, Ordering::AcqRel);
        }
        None if SLEEP_LOG_ON.load(Ordering::Acquire) => fail(SchedError::Trace),
        None => {}
    }
}

fn log_resume(task_id: u64) {
    if let Some(index) = sleep_record_of(task_id) {
        SLEEP_RESUMED_AT[index].store(timer::ticks(), Ordering::Release);
    }
}

/// Endort la tâche courante au moins `ms` millisecondes. Une durée nulle ne fait
/// que céder le CPU, la tâche restant `READY`.
pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        yield_now();
        return;
    }
    cpu::disable_interrupts();
    let start = timer::ticks();
    let deadline = timer_queue::deadline_for(start, ms, timer::TIMER_HZ);
    if let Err(error) = try_sleep(start, deadline) {
        fail(error);
    }
    // Reprise après l'échéance ou un `wake()` : la tâche est RUNNING sur son CPU.
    cpu::enable_interrupts();
}

fn try_sleep(start: u64, deadline: u64) -> Result<(), SchedError> {
    let cpu = this_cpu();
    let current = CURRENT[cpu].load(Ordering::Acquire);
    if current == BSP_CONTEXT {
        return Err(SchedError::NotInTaskContext);
    }
    // Passage à SLEEPING, inscription de l'échéance et choix de la suivante sous
    // le verrou de la file : le tick, qui doit le prendre pour remettre la tâche
    // en file, ne peut pas la réveiller avant qu'elle ait quitté le CPU.
    let next = {
        let mut guard = queue();
        task::sleep(task::lookup(current)?)?;
        log_sleep(current, start, deadline)?;
        timers()
            .insert(current, deadline)
            .map_err(SchedError::Timers)?;
        run_queue::exit_next(&mut guard)
    };
    SLEEPS.fetch_add(1, Ordering::AcqRel);
    switch_away(cpu, current, next)
}

/// Appelé à chaque tick du BSP, IRQ masquées : réveille chaque tâche dont
/// l'échéance est atteinte. Le verrou temporel est relâché avant chaque réveil.
fn expire_sleepers(now: u64) {
    loop {
        let expired = timers().pop_expired(now);
        let Some(task_id) = expired else {
            return;
        };
        match wake_task(task_id) {
            Ok(WakeOutcome::Woken(_)) => {
                log_wake(task_id, now, false);
                TIMER_WAKES.fetch_add(1, Ordering::AcqRel);
            }
            // Échéance d'une tâche qui ne dort plus : un réveil explicite
            // concurrent l'a emporté. Toute qualification exige 0 hors course
            // construite.
            Ok(WakeOutcome::NotAsleep) => {
                STALE_EXPIRIES.fetch_add(1, Ordering::AcqRel);
            }
            Err(error) => fail(error),
        }
    }
}

/// Attend, IRQ actives, que `condition` soit vraie, au plus `max_ticks` ticks.
/// Retourne IRQ masquées.
fn wait_ticks_until(max_ticks: u64, mut condition: impl FnMut() -> bool) -> Result<(), SchedError> {
    let limit = timer::ticks().saturating_add(max_ticks);
    loop {
        cpu::disable_interrupts();
        if condition() {
            return Ok(());
        }
        if timer::ticks() > limit {
            return Err(SchedError::SleepTimeout);
        }
        cpu::wait_for_interrupt();
    }
}

// --- K004-09 : qualification ------------------------------------------------------
//
// Les AP sont dans leur idle task, le timer du BSP tourne à TIMER_HZ :
// a. trois tâches dorment 30, 10 et 20 ms et se réveillent dans l'ordre des
//    échéances, jamais en avance ;
// b. deux tâches sur l'AP 1 : la seconde tourne pendant que la première dort,
//    puis l'AP repasse en idle quand les deux dorment ;
// c. une tâche endormie 300 ms est réveillée explicitement par le BSP ; son
//    échéance annulée ne la réveille pas une seconde fois ;
// d. `sleep_ms(0)` cède le CPU sans endormir la tâche.

const SLEEP_ARG_BASE: u64 = 0xE1_0409_0000_0000;
const SLEEP_ROLE_TIMED: u64 = 1;
const SLEEP_ROLE_SLEEPER: u64 = 2;
const SLEEP_ROLE_RUNNER: u64 = 3;
const SLEEP_ROLE_EXPLICIT: u64 = 4;
const SLEEP_ROLE_ZERO: u64 = 5;
const TIMED_MS: [u64; 3] = [30, 10, 20];
const SHARED_SLEEPER_MS: u64 = 50;
const SHARED_RUNNER_MS: u64 = 60;
const EXPLICIT_MS: u64 = 300;
/// Ticks d'attente avant le réveil explicite, bien avant l'échéance.
const EXPLICIT_AFTER_TICKS: u64 = 5;
/// Retard maximal toléré d'un réveil par le timer, en ticks.
pub const SLEEP_MAX_LATE: u64 = 2;
const SLEEP_TASKS: usize = 7;
/// Attente maximale d'une phase : 3 s à 100 Hz.
const SLEEP_PHASE_TICKS: u64 = 300;

static SLEEP_DONE: AtomicU64 = AtomicU64::new(0);
static SHARED_SLEEPER: AtomicU64 = AtomicU64::new(0);
static RAN_WHILE_SLEEPING: AtomicBool = AtomicBool::new(false);
static ZERO_STAYED_AWAKE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub struct TimedSleep {
    pub task: u64,
    pub cpu: usize,
    pub ms: u64,
    pub start: u64,
    pub deadline: u64,
    pub woke_at: u64,
    pub resumed_at: u64,
}

pub struct SleepQualification {
    pub cpus: usize,
    pub timed: [TimedSleep; 3],
    pub shared_cpu: usize,
    pub sleeper: u64,
    pub runner: u64,
    pub idle_entries: u64,
    pub explicit: TimedSleep,
    pub zero_task: u64,
    pub sleeps: u64,
    pub timer_wakes: u64,
    pub explicit_wakes: u64,
    pub stale_expiries: u64,
    pub max_late: u64,
    pub ipi_sent: u64,
    pub ipi_received: u64,
    pub idle_running_aps: usize,
    pub running_max: usize,
    pub canaries_ok: usize,
}

/// `Ok(None)` sur une machine sans AP.
pub fn qualify_sleep(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
) -> Result<Option<SleepQualification>, SchedError> {
    cpu::disable_interrupts();
    if this_cpu() != 0 {
        return Err(SchedError::UnknownCpu);
    }
    let cpus = online_mask().count_ones() as usize;
    if cpus < 2 {
        return Ok(None);
    }
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let ap = |index: usize| 1 + index % (cpus - 1);
    let entry = elyon_sleep_task_entry as *const () as usize as u64;
    let arg = |role: u64, ms: u64| SLEEP_ARG_BASE | (role << 16) | ms;

    for counter in [
        &SLEEPS,
        &TIMER_WAKES,
        &EXPLICIT_WAKES,
        &STALE_EXPIRIES,
        &SLEEP_DONE,
        &RESCHED_SENT,
        &RESCHED_RECEIVED,
    ] {
        counter.store(0, Ordering::Release);
    }
    SLEEP_LOG_LEN.store(0, Ordering::Release);
    SLEEP_LOG_ON.store(true, Ordering::Release);
    RUNNING_MAX.store(0, Ordering::Release);
    RAN_WHILE_SLEEPING.store(false, Ordering::Release);
    ZERO_STAYED_AWAKE.store(false, Ordering::Release);
    let mut regions = [StackRegion { start: 0, end: 0 }; SLEEP_TASKS];
    let mut spawned = 0usize;
    let mut spawn = |cpu: usize, task_arg: u64| -> Result<u64, SchedError> {
        let (id, region) = spawn_on(1 << cpu, entry, task_arg, bsp, identity_limit)?;
        regions[spawned] = region;
        spawned += 1;
        Ok(id)
    };

    timer::restart_periodic(timer_initial_count)?;

    // a. Trois durées distinctes, sur des AP différents quand il y en a assez.
    let mut timed_ids = [0u64; 3];
    for (index, &ms) in TIMED_MS.iter().enumerate() {
        timed_ids[index] = spawn(ap(index), arg(SLEEP_ROLE_TIMED, ms))?;
        enqueue(timed_ids[index], true)?;
    }
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        SLEEP_DONE.load(Ordering::Acquire) == 3
    })?;

    // b. Deux tâches sur l'AP 1 : la première s'endort, la seconde tourne.
    let shared_cpu = ap(0);
    let idle_before = IDLE_ENTRIES[shared_cpu].load(Ordering::Acquire);
    let sleeper = spawn(shared_cpu, arg(SLEEP_ROLE_SLEEPER, SHARED_SLEEPER_MS))?;
    let runner = spawn(shared_cpu, arg(SLEEP_ROLE_RUNNER, SHARED_RUNNER_MS))?;
    SHARED_SLEEPER.store(sleeper, Ordering::Release);
    enqueue(sleeper, false)?;
    enqueue(runner, true)?;
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        SLEEP_DONE.load(Ordering::Acquire) == 5
    })?;
    let idle_entries = IDLE_ENTRIES[shared_cpu].load(Ordering::Acquire) - idle_before;

    // c. Réveil explicite bien avant l'échéance, puis attente au-delà de celle-ci.
    let explicit_id = spawn(ap(1), arg(SLEEP_ROLE_EXPLICIT, EXPLICIT_MS))?;
    enqueue(explicit_id, true)?;
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        task::lookup(explicit_id)
            .and_then(task::snapshot)
            .is_ok_and(|snapshot| snapshot.state == TaskState::Sleeping)
    })?;
    let asleep_at = timer::ticks();
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        timer::ticks() >= asleep_at + EXPLICIT_AFTER_TICKS
    })?;
    if !matches!(wake(explicit_id)?, WakeOutcome::Woken(_)) {
        return Err(SchedError::Trace);
    }
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        SLEEP_DONE.load(Ordering::Acquire) == 6
    })?;
    let explicit_deadline = sleep_record_of(explicit_id)
        .map(|index| SLEEP_DEADLINE[index].load(Ordering::Acquire))
        .ok_or(SchedError::Trace)?;
    wait_ticks_until(SLEEP_PHASE_TICKS * 2, || {
        timer::ticks() > explicit_deadline + SLEEP_MAX_LATE + 1
    })?;

    // d. Sommeil nul.
    let zero_task = spawn(ap(2), arg(SLEEP_ROLE_ZERO, 0))?;
    enqueue(zero_task, true)?;
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        SLEEP_DONE.load(Ordering::Acquire) == 7
    })?;

    // Les AP reviennent dans leur idle task, puis marge pour un IPI encore en vol.
    let idles: [u64; MAX_SCHED_CPUS] =
        core::array::from_fn(|index| IDLE_IDS[index].load(Ordering::Acquire));
    wait_ticks_until(SLEEP_PHASE_TICKS, || {
        (1..cpus).all(|index| CURRENT[index].load(Ordering::Acquire) == idles[index])
    })?;
    let settled_from = timer::ticks();
    wait_ticks_until(SLEEP_PHASE_TICKS, || timer::ticks() >= settled_from + 2)?;
    timer::stop()?;
    SLEEP_LOG_ON.store(false, Ordering::Release);

    let timers_empty = timers().is_empty();
    if !timers_empty {
        return Err(SchedError::TimerQueueNotEmpty);
    }

    let len = SLEEP_LOG_LEN.load(Ordering::Acquire);
    if len > SLEEP_LOG_CAPACITY {
        return Err(SchedError::TraceOverflow);
    }
    let mut records = [SleepRecord {
        task: 0,
        deadline: 0,
        woke_at: 0,
        wakes: 0,
        explicit: false,
    }; SLEEP_LOG_CAPACITY];
    for (index, record) in records.iter_mut().enumerate().take(len) {
        *record = SleepRecord {
            task: SLEEP_TASK[index].load(Ordering::Acquire),
            deadline: SLEEP_DEADLINE[index].load(Ordering::Acquire),
            woke_at: SLEEP_WOKE_AT[index].load(Ordering::Acquire),
            wakes: SLEEP_WAKES[index].load(Ordering::Acquire) as u32,
            explicit: SLEEP_EXPLICIT[index].load(Ordering::Acquire),
        };
    }
    timer_queue::check_sleep(&records[..len], SLEEP_MAX_LATE).map_err(|error| {
        kprintln!("[sched-sleep] trace_error={:?}", error);
        SchedError::Trace
    })?;

    let record = |task_id: u64, ms: u64, cpu: usize| -> Result<TimedSleep, SchedError> {
        let index = sleep_record_of(task_id).ok_or(SchedError::Trace)?;
        Ok(TimedSleep {
            task: task_id,
            cpu,
            ms,
            start: SLEEP_START[index].load(Ordering::Acquire),
            deadline: SLEEP_DEADLINE[index].load(Ordering::Acquire),
            woke_at: SLEEP_WOKE_AT[index].load(Ordering::Acquire),
            resumed_at: SLEEP_RESUMED_AT[index].load(Ordering::Acquire),
        })
    };
    let timed = [
        record(timed_ids[0], TIMED_MS[0], ap(0))?,
        record(timed_ids[1], TIMED_MS[1], ap(1))?,
        record(timed_ids[2], TIMED_MS[2], ap(2))?,
    ];
    let explicit = record(explicit_id, EXPLICIT_MS, ap(1))?;
    let explicit_flagged = sleep_record_of(explicit_id)
        .is_some_and(|index| SLEEP_EXPLICIT[index].load(Ordering::Acquire));
    if !explicit_flagged {
        return Err(SchedError::Trace);
    }
    let max_late = records[..len]
        .iter()
        .filter(|record| !record.explicit)
        .map(|record| record.woke_at - record.deadline)
        .max()
        .unwrap_or(0);

    for id in [
        timed_ids[0],
        timed_ids[1],
        timed_ids[2],
        sleeper,
        runner,
        explicit_id,
        zero_task,
    ] {
        if task::snapshot(task::lookup(id)?)?.state != TaskState::Dead {
            return Err(SchedError::TaskNotDead);
        }
    }
    let mut idle_running_aps = 0;
    for (index, &idle) in idles.iter().enumerate().take(cpus).skip(1) {
        let snapshot = task::snapshot(task::lookup(idle)?)?;
        if snapshot.state != TaskState::Running || snapshot.current_cpu != index as u32 {
            return Err(SchedError::IdleState);
        }
        idle_running_aps += 1;
    }

    let sleeps = SLEEPS.load(Ordering::Acquire);
    let timer_wakes = TIMER_WAKES.load(Ordering::Acquire);
    let explicit_wakes = EXPLICIT_WAKES.load(Ordering::Acquire);
    let stale_expiries = STALE_EXPIRIES.load(Ordering::Acquire);
    let ipi_sent = RESCHED_SENT.load(Ordering::Acquire);
    let ipi_received = RESCHED_RECEIVED.load(Ordering::Acquire);
    // Six mises en file prévenues (la tâche endormie de b ne l'est pas, celle qui
    // tourne l'est), cinq réveils par le timer et un réveil explicite, tous vers
    // un AP.
    if sleeps != 6
        || timer_wakes != 5
        || explicit_wakes != 1
        || stale_expiries != 0
        || !ipi_counts_consistent(ipi_sent, ipi_received, 12)
        || idle_entries != 1
        || !RAN_WHILE_SLEEPING.load(Ordering::Acquire)
        || !ZERO_STAYED_AWAKE.load(Ordering::Acquire)
    {
        kprintln!(
            "[sched-sleep] counts sleeps={} timer_wakes={} explicit_wakes={} stale={} ipi_sent={} ipi_received={} idle_entries={}",
            sleeps,
            timer_wakes,
            explicit_wakes,
            stale_expiries,
            ipi_sent,
            ipi_received,
            idle_entries
        );
        return Err(SchedError::CountMismatch);
    }

    let mut canaries_ok = 0;
    for region in regions.iter().take(spawned) {
        if !stacks::canary_intact_at(*region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    Ok(Some(SleepQualification {
        cpus,
        timed,
        shared_cpu,
        sleeper,
        runner,
        idle_entries,
        explicit,
        zero_task,
        sleeps,
        timer_wakes,
        explicit_wakes,
        stale_expiries,
        max_late,
        ipi_sent,
        ipi_received,
        idle_running_aps,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        canaries_ok,
    }))
}

#[unsafe(no_mangle)]
extern "C" fn elyon_sleep_task_entry(arg: u64) -> ! {
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
    if arg & !0xFFFF_FFFF != SLEEP_ARG_BASE {
        fail(SchedError::ArgMismatch);
    }
    let role = (arg >> 16) & 0xFFFF;
    let ms = arg & 0xFFFF;
    match role {
        SLEEP_ROLE_TIMED | SLEEP_ROLE_SLEEPER | SLEEP_ROLE_EXPLICIT => {
            sleep_ms(ms);
            log_resume(current);
        }
        SLEEP_ROLE_RUNNER => {
            let sleeper = SHARED_SLEEPER.load(Ordering::Acquire);
            let asleep = task::lookup(sleeper)
                .and_then(task::snapshot)
                .is_ok_and(|snapshot| snapshot.state == TaskState::Sleeping);
            RAN_WHILE_SLEEPING.store(asleep, Ordering::Release);
            sleep_ms(ms);
            log_resume(current);
        }
        SLEEP_ROLE_ZERO => {
            let before = SLEEPS.load(Ordering::Acquire);
            sleep_ms(0);
            let still_running = task::lookup(current)
                .and_then(task::snapshot)
                .is_ok_and(|snapshot| snapshot.state == TaskState::Running);
            ZERO_STAYED_AWAKE.store(
                still_running && SLEEPS.load(Ordering::Acquire) == before,
                Ordering::Release,
            );
        }
        _ => fail(SchedError::ArgMismatch),
    }
    SLEEP_DONE.fetch_add(1, Ordering::AcqRel);
    exit_now()
}

// --- K004-10 : qualification de la synchronisation --------------------------------
//
// Les AP sont dans leur idle task, le timer du BSP tourne :
// a. compteur partagé : deux tâches par AP l'incrémentent par lecture puis
//    écriture non atomiques, sous un verrou de classe Data, en cédant le CPU et
//    en dormant régulièrement ; le total doit être exact ;
// b. PMM concurrent : une tâche par AP alloue une page, y écrit sa marque, cède
//    le CPU, la relit et la libère ; en même temps que (a) ;
// c. ping-pong : deux tâches sur deux AP se réveillent mutuellement depuis leur
//    AP, chacune se bloquant après avoir réveillé l'autre ;
// d. course au réveil : trois réveils simultanés (deux AP et le BSP) d'une même
//    tâche bloquée ; un seul gagne, la tâche ne reprend qu'une fois.
// Pendant tout ce temps, chaque prise de verrou vérifie l'ordre des classes et
// chaque switch l'absence de verrou tenu (`crate::sync`, fail-closed).

const SYNC_ARG_BASE: u64 = 0xE1_0410_0000_0000;
const SYNC_ROLE_COUNTER: u64 = 1;
const SYNC_ROLE_PMM: u64 = 2;
const SYNC_ROLE_PING: u64 = 3;
const SYNC_ROLE_PONG: u64 = 4;
const SYNC_ROLE_TARGET: u64 = 5;
const SYNC_ROLE_WAKER: u64 = 6;
const COUNTER_TASKS_PER_AP: usize = 2;
pub const COUNTER_ROUNDS: u64 = 20_000;
pub const PMM_ROUNDS: u64 = 64;
pub const PING_ROUNDS: u64 = 50;
/// 3 tâches par AP (jusqu'à 7 AP), plus le ping-pong et la course au réveil.
const SYNC_MAX_TASKS: usize = 32;
/// Attente maximale d'une phase : 10 s à 100 Hz.
const SYNC_PHASE_TICKS: u64 = 1_000;

/// Compteur partagé : volontairement mis à jour par load puis store ; seul
/// COUNTER_LOCK empêche les mises à jour perdues.
static SHARED_COUNTER: AtomicU64 = AtomicU64::new(0);
static COUNTER_LOCK: IrqSpinLock<()> = IrqSpinLock::new(LockClass::Data, ());
static SYNC_DONE: AtomicU64 = AtomicU64::new(0);
static PMM_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static PMM_DUPLICATES: AtomicU64 = AtomicU64::new(0);
static PMM_FAILURES: AtomicU64 = AtomicU64::new(0);
static PING_ID: AtomicU64 = AtomicU64::new(0);
static PONG_ID: AtomicU64 = AtomicU64::new(0);
static PINGPONG_WAKES: AtomicU64 = AtomicU64::new(0);
static PINGPONG_RETRIES: AtomicU64 = AtomicU64::new(0);
static WAKE_TARGET: AtomicU64 = AtomicU64::new(0);
static WAKE_GO: AtomicBool = AtomicBool::new(false);
static WAKERS_READY: AtomicU64 = AtomicU64::new(0);
static WAKE_WINS: AtomicU64 = AtomicU64::new(0);
static WAKE_LOSSES: AtomicU64 = AtomicU64::new(0);
static TARGET_RUNS: AtomicU64 = AtomicU64::new(0);

pub struct SyncQualification {
    pub cpus: usize,
    pub counter_tasks: u64,
    pub counter_total: u64,
    pub pmm_tasks: u64,
    pub pmm_allocations: u64,
    pub pmm_duplicates: u64,
    pub pmm_failures: u64,
    pub free_before: usize,
    pub free_after: usize,
    pub ping_cpu: usize,
    pub pong_cpu: usize,
    pub pingpong_wakes: u64,
    pub pingpong_retries: u64,
    pub wake_target: u64,
    pub wake_wins: u64,
    pub wake_losses: u64,
    pub target_runs: u64,
    pub acquisitions: [u64; 4],
    pub contention: [u64; 4],
    pub switch_checks: u64,
    pub ipi_sent: u64,
    pub ipi_received: u64,
    pub idle_running_aps: usize,
    pub running_max: usize,
    pub canaries_ok: usize,
}

/// `Ok(None)` sur une machine sans AP.
pub fn qualify_sync(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
    timer_initial_count: u32,
) -> Result<Option<SyncQualification>, SchedError> {
    cpu::disable_interrupts();
    if this_cpu() != 0 {
        return Err(SchedError::UnknownCpu);
    }
    let cpus = online_mask().count_ones() as usize;
    if cpus < 2 {
        return Ok(None);
    }
    let aps = cpus - 1;
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let ap = |index: usize| 1 + index % aps;
    let entry = elyon_sync_task_entry as *const () as usize as u64;
    let arg = |role: u64| SYNC_ARG_BASE | (role << 16);

    for counter in [
        &SHARED_COUNTER,
        &SYNC_DONE,
        &PMM_ALLOCATIONS,
        &PMM_DUPLICATES,
        &PMM_FAILURES,
        &PINGPONG_WAKES,
        &PINGPONG_RETRIES,
        &WAKERS_READY,
        &WAKE_WINS,
        &WAKE_LOSSES,
        &TARGET_RUNS,
        &RESCHED_SENT,
        &RESCHED_RECEIVED,
    ] {
        counter.store(0, Ordering::Release);
    }
    WAKE_GO.store(false, Ordering::Release);
    RUNNING_MAX.store(0, Ordering::Release);
    let switch_checks_before = crate::sync::switch_checks();
    let acquisitions_before = LockClass::ALL.map(crate::sync::acquisitions);
    let contention_before = LockClass::ALL.map(crate::sync::contention);

    let mut regions = [StackRegion { start: 0, end: 0 }; SYNC_MAX_TASKS];
    let mut ids = [0u64; SYNC_MAX_TASKS];
    let mut spawned = 0usize;
    let mut spawn = |cpu: usize, role: u64| -> Result<u64, SchedError> {
        if spawned == SYNC_MAX_TASKS {
            return Err(SchedError::TraceOverflow);
        }
        let (id, region) = spawn_on(1 << cpu, entry, arg(role), bsp, identity_limit)?;
        regions[spawned] = region;
        ids[spawned] = id;
        spawned += 1;
        Ok(id)
    };

    timer::restart_periodic(timer_initial_count)?;

    // a + b. Compteur et PMM, en même temps sur tous les AP. Les piles sont
    // allouées avant la mesure de référence du PMM.
    let counter_tasks = (aps * COUNTER_TASKS_PER_AP) as u64;
    let mut phase_one = [0u64; SYNC_MAX_TASKS];
    let mut phase_one_len = 0;
    for index in 0..aps {
        for _ in 0..COUNTER_TASKS_PER_AP {
            phase_one[phase_one_len] = spawn(ap(index), SYNC_ROLE_COUNTER)?;
            phase_one_len += 1;
        }
        phase_one[phase_one_len] = spawn(ap(index), SYNC_ROLE_PMM)?;
        phase_one_len += 1;
    }
    let free_before = crate::memory::free_frame_count();
    for &id in &phase_one[..phase_one_len] {
        enqueue(id, true)?;
    }
    let phase_one_tasks = phase_one_len as u64;
    wait_ticks_until(SYNC_PHASE_TICKS, || {
        SYNC_DONE.load(Ordering::Acquire) == phase_one_tasks
    })?;
    let free_after = crate::memory::free_frame_count();

    // c. Ping-pong entre deux AP (le même s'il n'y en a qu'un).
    let ping_cpu = ap(0);
    let pong_cpu = ap(1);
    let pong = spawn(pong_cpu, SYNC_ROLE_PONG)?;
    let ping = spawn(ping_cpu, SYNC_ROLE_PING)?;
    PING_ID.store(ping, Ordering::Release);
    PONG_ID.store(pong, Ordering::Release);
    enqueue(pong, true)?;
    enqueue(ping, true)?;
    wait_ticks_until(SYNC_PHASE_TICKS, || {
        SYNC_DONE.load(Ordering::Acquire) == phase_one_tasks + 2
    })?;

    // d. Course au réveil : la cible se bloque, deux réveilleurs attendent le
    // signal sur deux AP, le BSP donne le signal et réveille aussi.
    let target = spawn(ap(0), SYNC_ROLE_TARGET)?;
    WAKE_TARGET.store(target, Ordering::Release);
    let wakers = [
        spawn(ap(1), SYNC_ROLE_WAKER)?,
        spawn(ap(2), SYNC_ROLE_WAKER)?,
    ];
    enqueue(target, true)?;
    for waker in wakers {
        enqueue(waker, true)?;
    }
    wait_ticks_until(SYNC_PHASE_TICKS, || {
        WAKERS_READY.load(Ordering::Acquire) == 2
            && task::lookup(target)
                .and_then(task::snapshot)
                .is_ok_and(|snapshot| snapshot.state == TaskState::Blocked)
    })?;
    WAKE_GO.store(true, Ordering::Release);
    match wake(target)? {
        WakeOutcome::Woken(_) => WAKE_WINS.fetch_add(1, Ordering::AcqRel),
        WakeOutcome::NotAsleep => WAKE_LOSSES.fetch_add(1, Ordering::AcqRel),
    };
    wait_ticks_until(SYNC_PHASE_TICKS, || {
        SYNC_DONE.load(Ordering::Acquire) == phase_one_tasks + 5
    })?;

    // Les AP reviennent dans leur idle task, puis marge pour un IPI en vol.
    let idles: [u64; MAX_SCHED_CPUS] =
        core::array::from_fn(|index| IDLE_IDS[index].load(Ordering::Acquire));
    wait_ticks_until(SYNC_PHASE_TICKS, || {
        (1..cpus).all(|index| CURRENT[index].load(Ordering::Acquire) == idles[index])
    })?;
    let settled_from = timer::ticks();
    wait_ticks_until(SYNC_PHASE_TICKS, || timer::ticks() >= settled_from + 2)?;
    timer::stop()?;

    let counter_total = SHARED_COUNTER.load(Ordering::Acquire);
    let pmm_allocations = PMM_ALLOCATIONS.load(Ordering::Acquire);
    let pmm_duplicates = PMM_DUPLICATES.load(Ordering::Acquire);
    let pmm_failures = PMM_FAILURES.load(Ordering::Acquire);
    let pingpong_wakes = PINGPONG_WAKES.load(Ordering::Acquire);
    let wake_wins = WAKE_WINS.load(Ordering::Acquire);
    let wake_losses = WAKE_LOSSES.load(Ordering::Acquire);
    let target_runs = TARGET_RUNS.load(Ordering::Acquire);
    let ipi_sent = RESCHED_SENT.load(Ordering::Acquire);
    let ipi_received = RESCHED_RECEIVED.load(Ordering::Acquire);
    let pmm_tasks = aps as u64;
    let consistent = counter_total == counter_tasks * COUNTER_ROUNDS
        && pmm_allocations == pmm_tasks * PMM_ROUNDS
        && pmm_duplicates == 0
        && pmm_failures == 0
        && free_after == free_before
        && pingpong_wakes == 2 * PING_ROUNDS
        && wake_wins == 1
        && wake_losses == 2
        && target_runs == 1
        && ipi_sent >= 1
        && (1..=ipi_sent).contains(&ipi_received);
    if !consistent {
        kprintln!(
            "[sched-sync] counts counter={} pmm={} dup={} fail={} free={}->{} pingpong={} wins={} losses={} runs={} ipi={}/{}",
            counter_total,
            pmm_allocations,
            pmm_duplicates,
            pmm_failures,
            free_before,
            free_after,
            pingpong_wakes,
            wake_wins,
            wake_losses,
            target_runs,
            ipi_received,
            ipi_sent
        );
        return Err(SchedError::CountMismatch);
    }

    for &id in &ids[..spawned] {
        if task::snapshot(task::lookup(id)?)?.state != TaskState::Dead {
            return Err(SchedError::TaskNotDead);
        }
    }
    let mut idle_running_aps = 0;
    for (index, &idle) in idles.iter().enumerate().take(cpus).skip(1) {
        let snapshot = task::snapshot(task::lookup(idle)?)?;
        if snapshot.state != TaskState::Running || snapshot.current_cpu != index as u32 {
            return Err(SchedError::IdleState);
        }
        idle_running_aps += 1;
    }
    let mut canaries_ok = 0;
    for region in regions.iter().take(spawned) {
        if !stacks::canary_intact_at(*region) {
            return Err(SchedError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    let mut acquisitions = [0u64; 4];
    let mut contention = [0u64; 4];
    for (index, class) in LockClass::ALL.iter().enumerate() {
        acquisitions[index] = crate::sync::acquisitions(*class) - acquisitions_before[index];
        contention[index] = crate::sync::contention(*class) - contention_before[index];
    }

    Ok(Some(SyncQualification {
        cpus,
        counter_tasks,
        counter_total,
        pmm_tasks,
        pmm_allocations,
        pmm_duplicates,
        pmm_failures,
        free_before,
        free_after,
        ping_cpu,
        pong_cpu,
        pingpong_wakes,
        pingpong_retries: PINGPONG_RETRIES.load(Ordering::Acquire),
        wake_target: target,
        wake_wins,
        wake_losses,
        target_runs,
        acquisitions,
        contention,
        switch_checks: crate::sync::switch_checks() - switch_checks_before,
        ipi_sent,
        ipi_received,
        idle_running_aps,
        running_max: RUNNING_MAX.load(Ordering::Acquire),
        canaries_ok,
    }))
}

/// Réveille `task_id` dès qu'elle est bloquée : un réveil arrivé avant que la
/// tâche ne se bloque ne la trouve pas endormie et serait perdu.
const WAKE_RETRY_LIMIT: u64 = 1_000_000;

fn wake_when_blocked(task_id: u64) {
    for _ in 0..WAKE_RETRY_LIMIT {
        match wake(task_id) {
            Ok(WakeOutcome::Woken(_)) => {
                PINGPONG_WAKES.fetch_add(1, Ordering::AcqRel);
                return;
            }
            Ok(WakeOutcome::NotAsleep) => {
                PINGPONG_RETRIES.fetch_add(1, Ordering::Relaxed);
                yield_now();
            }
            Err(error) => fail(error),
        }
    }
    fail(SchedError::WakeRetriesExhausted)
}

fn counter_task() {
    for round in 0..COUNTER_ROUNDS {
        {
            let _guard = COUNTER_LOCK.lock();
            let value = SHARED_COUNTER.load(Ordering::Relaxed);
            core::hint::spin_loop();
            SHARED_COUNTER.store(value + 1, Ordering::Relaxed);
        }
        if round % 1_000 == 999 {
            yield_now();
        }
        if round % 5_000 == 4_999 {
            sleep_ms(10);
        }
    }
}

fn pmm_task(current: u64) {
    for round in 0..PMM_ROUNDS {
        let Some(frame) = crate::memory::allocate_frame() else {
            PMM_FAILURES.fetch_add(1, Ordering::AcqRel);
            continue;
        };
        PMM_ALLOCATIONS.fetch_add(1, Ordering::AcqRel);
        let tag = (current << 32) | round;
        // SAFETY: page identity-mappée que cette tâche vient d'obtenir du PMM ;
        // si un autre CPU l'avait aussi obtenue, la marque serait écrasée.
        unsafe { write_volatile(frame as *mut u64, tag) };
        yield_now();
        // SAFETY: même page, toujours détenue par cette tâche.
        if unsafe { read_volatile(frame as *const u64) } != tag {
            PMM_DUPLICATES.fetch_add(1, Ordering::AcqRel);
        }
        if !crate::memory::free_frame(frame) {
            PMM_FAILURES.fetch_add(1, Ordering::AcqRel);
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_sync_task_entry(arg: u64) -> ! {
    let current = CURRENT[this_cpu()].load(Ordering::Acquire);
    if arg & !0xFFFF_FFFF != SYNC_ARG_BASE {
        fail(SchedError::ArgMismatch);
    }
    match (arg >> 16) & 0xFFFF {
        SYNC_ROLE_COUNTER => counter_task(),
        SYNC_ROLE_PMM => pmm_task(current),
        SYNC_ROLE_PING => {
            let pong = PONG_ID.load(Ordering::Acquire);
            for _ in 0..PING_ROUNDS {
                wake_when_blocked(pong);
                block_now();
            }
        }
        SYNC_ROLE_PONG => {
            let ping = PING_ID.load(Ordering::Acquire);
            block_now();
            for round in 0..PING_ROUNDS {
                wake_when_blocked(ping);
                if round + 1 < PING_ROUNDS {
                    block_now();
                }
            }
        }
        SYNC_ROLE_TARGET => {
            block_now();
            TARGET_RUNS.fetch_add(1, Ordering::AcqRel);
        }
        SYNC_ROLE_WAKER => {
            WAKERS_READY.fetch_add(1, Ordering::AcqRel);
            while !WAKE_GO.load(Ordering::Acquire) {
                yield_now();
            }
            match wake(WAKE_TARGET.load(Ordering::Acquire)) {
                Ok(WakeOutcome::Woken(_)) => WAKE_WINS.fetch_add(1, Ordering::AcqRel),
                Ok(WakeOutcome::NotAsleep) => WAKE_LOSSES.fetch_add(1, Ordering::AcqRel),
                Err(error) => fail(error),
            };
        }
        _ => fail(SchedError::ArgMismatch),
    }
    SYNC_DONE.fetch_add(1, Ordering::AcqRel);
    exit_now()
}
