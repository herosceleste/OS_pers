//! K004-02 : pile PMM dédiée et contexte initial propre par tâche.
//!
//! Le BSP crée trois tâches noyau, attribue à chacune une pile de 64 KiB prise
//! au PMM, construit leur frame initial puis les exécute deux fois chacune en
//! coopératif. Chaque tâche relève son RSP et l'argument reçu ; le BSP vérifie
//! que tout reste dans la pile propre de la tâche et que les canaris tiennent.

use core::arch::global_asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::task_stack::{self, CANARY, CANARY_QWORDS, FrameSpec, StackError, StackRegion};

use crate::arch::x86_64::cpu;
use crate::task::{self, TaskError, TaskId, TaskState};
use crate::task_frames::{self, FramesError};
use crate::{context, kprintln, memory};

pub const QUALIFY_TASKS: usize = 3;
pub const ROUNDS: u64 = 2;

const STACK_FRAMES: usize = 16;
pub const STACK_SIZE: u64 = STACK_FRAMES as u64 * PAGE_SIZE;
const INITIAL_RFLAGS: u64 = 0x2;
const ARG_BASE: u64 = 0xE1_0402_0000;
const BSP_CPU: u32 = 0;
const BSP_AFFINITY: u64 = 1 << BSP_CPU;
const EXPECTED_SWITCHES: u64 = QUALIFY_TASKS as u64 * ROUNDS * 2;

global_asm!(
    r#"
.section .text.task_stack,"ax"
.balign 16

.global elyon_task_trampoline
elyon_task_trampoline:
    call elyon_finish_switch
    mov rdi, rbx
    call r12
    ud2
"#
);

unsafe extern "C" {
    fn elyon_task_trampoline();
}

static mut BSP_RSP: u64 = 0;
static mut TASK_RSP: [u64; QUALIFY_TASKS] = [0; QUALIFY_TASKS];

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static RUNS: [AtomicU64; QUALIFY_TASKS] = [const { AtomicU64::new(0) }; QUALIFY_TASKS];
static OBSERVED_ARG: [AtomicU64; QUALIFY_TASKS] = [const { AtomicU64::new(0) }; QUALIFY_TASKS];
static OBSERVED_RSP: [AtomicU64; QUALIFY_TASKS] = [const { AtomicU64::new(0) }; QUALIFY_TASKS];

#[derive(Clone, Copy)]
pub struct TaskStackReport {
    pub id: u64,
    pub stack: StackRegion,
    pub initial_rsp: u64,
    pub observed_rsp: u64,
    pub saved_rsp: u64,
    pub arg: u64,
}

pub struct StackQualification {
    pub tasks: [TaskStackReport; QUALIFY_TASKS],
    pub owned: usize,
    pub args_ok: usize,
    pub canaries_ok: usize,
    pub switches: u64,
}

#[derive(Clone, Copy)]
pub enum TaskStackError {
    Task(TaskError),
    Stack(StackError),
    Frames(FramesError),
    AllocationFailed,
    Overlap,
    NotOwned,
    NotRunning,
    RunCountMismatch,
    ArgMismatch,
    RspOutsideOwnStack,
    RspInsideOtherStack,
    StackOverflow,
    CanaryCorrupted,
    BspRspOutsideKernelStack,
    SwitchCountMismatch,
}

impl TaskStackError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task(error) => error.as_str(),
            Self::Stack(error) => error.as_str(),
            Self::Frames(error) => error.as_str(),
            Self::AllocationFailed => "PMM could not allocate a task stack",
            Self::Overlap => "task stacks overlap each other or the BSP stack",
            Self::NotOwned => "task registry does not record the expected stack",
            Self::NotRunning => "task is not RUNNING on the BSP before its switch",
            Self::RunCountMismatch => "task did not run the expected number of times",
            Self::ArgMismatch => "task received an unexpected entry argument",
            Self::RspOutsideOwnStack => "task RSP outside its own stack",
            Self::RspInsideOtherStack => "task RSP inside another task or BSP stack",
            Self::StackOverflow => "task RSP reached the canary zone",
            Self::CanaryCorrupted => "task stack canary overwritten",
            Self::BspRspOutsideKernelStack => "saved BSP RSP outside loader-provided kernel stack",
            Self::SwitchCountMismatch => "unexpected task context switch count",
        }
    }
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<StackQualification, TaskStackError> {
    cpu::disable_interrupts();

    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let entry = elyon_task_stack_entry as *const () as usize as u64;

    SWITCHES.store(0, Ordering::Release);
    for index in 0..QUALIFY_TASKS {
        RUNS[index].store(0, Ordering::Release);
        OBSERVED_ARG[index].store(0, Ordering::Release);
        OBSERVED_RSP[index].store(0, Ordering::Release);
    }

    let mut ids: [Option<TaskId>; QUALIFY_TASKS] = [None; QUALIFY_TASKS];
    let mut regions = [StackRegion { start: 0, end: 0 }; QUALIFY_TASKS];
    let mut initial_rsp = [0u64; QUALIFY_TASKS];

    for index in 0..QUALIFY_TASKS {
        let id = task::create_kernel(BSP_AFFINITY).map_err(TaskStackError::Task)?;
        let prepared = prepare(id, entry, ARG_BASE + index as u64, bsp, identity_limit)?;
        let region = prepared.region;
        let rsp = prepared.initial_rsp;

        // SAFETY: écriture d'un u64 statique, lu uniquement par le switch.
        unsafe { write_volatile(task_rsp_slot(index), rsp) };

        ids[index] = Some(id);
        regions[index] = region;
        initial_rsp[index] = rsp;
    }

    let mut all = [bsp; QUALIFY_TASKS + 1];
    all[1..].copy_from_slice(&regions);
    if task_stack::find_overlap(&all).is_some() {
        return Err(TaskStackError::Overlap);
    }

    let mut owned = 0;
    for index in 0..QUALIFY_TASKS {
        let id = ids[index].ok_or(TaskStackError::NotOwned)?;
        match task::stack_of(id).map_err(TaskStackError::Task)? {
            Some((start, end)) if start == regions[index].start && end == regions[index].end => {
                owned += 1;
            }
            _ => return Err(TaskStackError::NotOwned),
        }
    }

    let mut observed_rsp = [0u64; QUALIFY_TASKS];
    let mut args_ok = 0;

    for round in 1..=ROUNDS {
        for index in 0..QUALIFY_TASKS {
            let id = ids[index].ok_or(TaskStackError::NotOwned)?;

            task::start(id, BSP_CPU).map_err(TaskStackError::Task)?;
            let snapshot = task::snapshot(id).map_err(TaskStackError::Task)?;
            if snapshot.state != TaskState::Running || snapshot.current_cpu != BSP_CPU {
                return Err(TaskStackError::NotRunning);
            }

            CURRENT.store(index, Ordering::Release);
            run_task(index);
            task::yield_ready(id).map_err(TaskStackError::Task)?;

            if RUNS[index].load(Ordering::Acquire) != round {
                return Err(TaskStackError::RunCountMismatch);
            }
            if OBSERVED_ARG[index].load(Ordering::Acquire) != ARG_BASE + index as u64 {
                return Err(TaskStackError::ArgMismatch);
            }

            let observed = OBSERVED_RSP[index].load(Ordering::Acquire);
            // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
            let saved = unsafe { read_volatile(task_rsp_slot(index)) };
            check_rsp(index, observed, &regions, bsp)?;
            check_rsp(index, saved, &regions, bsp)?;

            if round == 1 {
                observed_rsp[index] = observed;
                args_ok += 1;
            }
        }
    }

    let mut canaries_ok = 0;
    for region in &regions {
        if !canary_intact_at(*region) {
            return Err(TaskStackError::CanaryCorrupted);
        }
        canaries_ok += 1;
    }

    // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
    let bsp_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };
    if !task_stack::rsp_inside(bsp, bsp_rsp) {
        return Err(TaskStackError::BspRspOutsideKernelStack);
    }

    let switches = SWITCHES.load(Ordering::Acquire);
    if switches != EXPECTED_SWITCHES {
        return Err(TaskStackError::SwitchCountMismatch);
    }

    let mut tasks = [TaskStackReport {
        id: 0,
        stack: StackRegion { start: 0, end: 0 },
        initial_rsp: 0,
        observed_rsp: 0,
        saved_rsp: 0,
        arg: 0,
    }; QUALIFY_TASKS];

    for index in 0..QUALIFY_TASKS {
        let id = ids[index].ok_or(TaskStackError::NotOwned)?;
        task::terminate(id).map_err(TaskStackError::Task)?;
        tasks[index] = TaskStackReport {
            id: id.raw(),
            stack: regions[index],
            initial_rsp: initial_rsp[index],
            observed_rsp: observed_rsp[index],
            // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
            saved_rsp: unsafe { read_volatile(task_rsp_slot(index)) },
            arg: OBSERVED_ARG[index].load(Ordering::Acquire),
        };
    }

    Ok(StackQualification {
        tasks,
        owned,
        args_ok,
        canaries_ok,
        switches,
    })
}

/// Pile prête à être exécutée : réservée, attribuée à la tâche, frame initial écrit.
#[derive(Clone, Copy)]
pub struct PreparedStack {
    pub region: StackRegion,
    pub initial_rsp: u64,
}

/// Réserve une pile de 64 KiB au PMM pour la tâche READY `id`, la met à zéro,
/// pose les canaris, écrit le frame initial (entrée `entry`, argument `arg`,
/// retour vers `elyon_task_trampoline`) et l'attribue à la tâche. La pile est
/// rendable : son jeton est gardé par le slot jusqu'à la récupération de la
/// tâche (K004-11).
pub fn prepare(
    id: TaskId,
    entry: u64,
    arg: u64,
    bsp: StackRegion,
    identity_limit: u64,
) -> Result<PreparedStack, TaskStackError> {
    let frames = memory::allocate_owned(STACK_FRAMES).ok_or(TaskStackError::AllocationFailed)?;
    let start = frames.start();
    // Dès maintenant, la pile appartient au slot : elle n'est rendue qu'à sa
    // récupération.
    task_frames::keep_stack(id, frames).map_err(TaskStackError::Frames)?;
    let region = StackRegion {
        start,
        end: start + STACK_SIZE,
    };

    task_stack::validate_region(region, STACK_SIZE, identity_limit)
        .map_err(TaskStackError::Stack)?;
    if task_stack::overlaps(region, bsp) {
        return Err(TaskStackError::Overlap);
    }

    // SAFETY: la région vient d'être réservée au PMM, validate_region garantit
    // qu'elle est entièrement mappée en identité, et aucune tâche ne s'y exécute.
    unsafe {
        write_bytes(start as *mut u8, 0, STACK_SIZE as usize);
        let bottom = start as *mut u64;
        for word in 0..CANARY_QWORDS {
            write_volatile(bottom.add(word), CANARY);
        }
    }

    let (rsp, frame) = task_stack::initial_frame(FrameSpec {
        stack: region,
        entry,
        arg,
        trampoline: elyon_task_trampoline as *const () as usize as u64,
        rflags: INITIAL_RFLAGS,
    })
    .map_err(TaskStackError::Stack)?;

    // SAFETY: `rsp` est le sommet de la pile réservée ci-dessus, aligné sur 16,
    // et le frame tient dans la pile (vérifié par initial_frame).
    unsafe {
        let slots = rsp as *mut u64;
        for (slot, value) in frame.iter().enumerate() {
            write_volatile(slots.add(slot), *value);
        }
    }

    task::attach_stack(id, region.start, region.end).map_err(TaskStackError::Task)?;

    Ok(PreparedStack {
        region,
        initial_rsp: rsp,
    })
}

/// Vrai si les canaris du bas de `region` portent encore leur motif.
/// À n'appeler que lorsqu'aucune tâche ne s'exécute sur cette pile.
pub fn canary_intact_at(region: StackRegion) -> bool {
    let mut words = [0u64; CANARY_QWORDS];
    // SAFETY: les piles de tâches sont mappées en identité (validate_region) et
    // l'appelant garantit qu'aucune tâche ne s'y exécute pendant la lecture.
    unsafe {
        let bottom = region.start as *const u64;
        for (word, value) in words.iter_mut().enumerate() {
            *value = read_volatile(bottom.add(word));
        }
    }
    task_stack::canary_intact(&words)
}

fn check_rsp(
    index: usize,
    rsp: u64,
    regions: &[StackRegion; QUALIFY_TASKS],
    bsp: StackRegion,
) -> Result<(), TaskStackError> {
    let own = regions[index];
    if !task_stack::rsp_inside(own, rsp) {
        let elsewhere = task_stack::rsp_inside(bsp, rsp)
            || regions
                .iter()
                .enumerate()
                .any(|(other, region)| other != index && task_stack::rsp_inside(*region, rsp));
        return Err(if elsewhere {
            TaskStackError::RspInsideOtherStack
        } else {
            TaskStackError::RspOutsideOwnStack
        });
    }
    if task_stack::rsp_overflowed(own, rsp) {
        return Err(TaskStackError::StackOverflow);
    }
    Ok(())
}

fn task_rsp_slot(index: usize) -> *mut u64 {
    // SAFETY: `index < QUALIFY_TASKS` pour tous les appelants ; on ne forme qu'un
    // pointeur brut, jamais de référence vers le `static mut`.
    unsafe { addr_of_mut!(TASK_RSP).cast::<u64>().add(index) }
}

fn run_task(index: usize) {
    SWITCHES.fetch_add(1, Ordering::AcqRel);
    // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
    let task_rsp = unsafe { read_volatile(task_rsp_slot(index)) };
    // SAFETY: `task_rsp` désigne le frame initial ou celui laissé par le dernier
    // `yield_to_bsp` de cette tâche, sur sa pile dédiée ; aucun verrou n'est tenu.
    unsafe { context::switch(addr_of_mut!(BSP_RSP), task_rsp) };
}

fn yield_to_bsp(index: usize) {
    SWITCHES.fetch_add(1, Ordering::AcqRel);
    // SAFETY: lecture d'un u64 statique écrit uniquement par le switch.
    let bsp_rsp = unsafe { read_volatile(addr_of!(BSP_RSP)) };
    // SAFETY: `bsp_rsp` est le frame laissé par `run_task` sur la pile du BSP,
    // qui attend ce retour ; aucun verrou n'est tenu.
    unsafe { context::switch(task_rsp_slot(index), bsp_rsp) };
}

#[unsafe(no_mangle)]
extern "C" fn elyon_task_stack_entry(arg: u64) -> ! {
    let index = CURRENT.load(Ordering::Acquire);

    for _ in 0..ROUNDS {
        OBSERVED_ARG[index].store(arg, Ordering::Release);
        OBSERVED_RSP[index].store(cpu::read_rsp(), Ordering::Release);
        RUNS[index].fetch_add(1, Ordering::AcqRel);
        yield_to_bsp(index);
    }

    kprintln!("[task-stack] FAILED task resumed after its final round");
    cpu::halt_forever()
}
