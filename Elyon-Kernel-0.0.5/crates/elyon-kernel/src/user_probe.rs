//! K005-02 : sonde ring 3 et `swapgs` à chaque entrée depuis le ring 3.
//!
//! Sur chaque AP, à tour de rôle, une tâche noyau entre en ring 3 dans une
//! boucle de quelques instructions, avec un GS utilisateur propre à la sonde.
//! Le BSP lui envoie des IPI de rescheduling : chaque IPI interrompt le ring 3,
//! et le stub d'entrée doit avoir fait `swapgs` avant que le noyau ne lise la
//! zone per-CPU. Le dispatcher vérifie, sans passer par GS, que GS_BASE est la
//! zone de ce CPU et que KERNEL_GS_BASE garde le GS utilisateur ; la sortie
//! vers le ring 3 refait `swapgs`, sinon la boucle utilisateur reprendrait
//! avec le GS du noyau. Quand assez d'entrées ont été vues, le noyau lève un
//! drapeau dans la page de pile utilisateur ; la sonde sort par `int 0x81`
//! (porte DPL 3), dont le dispatcher refait les mêmes contrôles et termine la
//! tâche.

use core::arch::{asm, global_asm};
use core::ptr::{copy_nonoverlapping, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::ring3::{self, Ring3Error};
use elyon_core::task_stack::StackRegion;

use crate::arch::x86_64::{cpu, cpu_local, idt, paging};
use crate::sched::{self, ReapOutcome, SchedError};
use crate::task::{self, TaskState};
use crate::{memory, timer};

const IA32_GS_BASE: u32 = 0xC000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
const CR4_FSGSBASE: u64 = 1 << 16;
/// Entrées depuis le ring 3 à observer avant de libérer la sonde.
pub const TARGET_ENTRIES: u64 = 20;
/// Délais de 1 ms entre deux IPI, au plus, par AP.
const MAX_IPI_ROUNDS: u32 = 5_000;
const REAP_SPINS: u64 = 50_000_000;
const PROBE_ARG_BASE: u64 = 0xE1_0502_0000_0000;
const MAX_PROBED_CPUS: usize = sched::MAX_SCHED_CPUS;

global_asm!(
    r#"
.section .rodata.user_probe,"a"
// Code ring 3 de la sonde, copié dans la page de code utilisateur : boucle
// jusqu'à ce que le noyau lève le drapeau au bas de la page de pile, puis
// sort par `int 0x81`. Indépendant de sa position, aucune pile utilisée.
.global elyon_user_probe_code_start
elyon_user_probe_code_start:
    movabs rax, {flag}
1:
    pause
    cmp qword ptr [rax], 0
    je 1b
    int {exit_vector}
    ud2
.global elyon_user_probe_code_end
elyon_user_probe_code_end:
"#,
    flag = const ring3::USER_STACK_VA,
    exit_vector = const idt::USER_PROBE_EXIT_VECTOR,
);

unsafe extern "C" {
    static elyon_user_probe_code_start: u8;
    static elyon_user_probe_code_end: u8;
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static PROBE_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
static EXPECTED_USER_GS: AtomicU64 = AtomicU64::new(0);
static FLAG_FRAME: AtomicU64 = AtomicU64::new(0);
static USER_ENTRIES: AtomicU64 = AtomicU64::new(0);
static GS_FAILURES: AtomicU64 = AtomicU64::new(0);
static EXITS: AtomicU64 = AtomicU64::new(0);
static EXIT_GS_OK: AtomicU64 = AtomicU64::new(0);
static FSGSBASE_ON: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub enum ProbeError {
    Sched(SchedError),
    Ring3(Ring3Error),
    AllocationFailed,
    /// La sonde n'est pas sortie du ring 3 dans le délai.
    Timeout,
}

impl ProbeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sched(error) => error.as_str(),
            Self::Ring3(error) => error.as_str(),
            Self::AllocationFailed => "PMM could not allocate the ring 3 probe pages",
            Self::Timeout => "ring 3 probe did not exit in time",
        }
    }
}

impl From<SchedError> for ProbeError {
    fn from(error: SchedError) -> Self {
        Self::Sched(error)
    }
}

impl From<Ring3Error> for ProbeError {
    fn from(error: Ring3Error) -> Self {
        Self::Ring3(error)
    }
}

/// Résultat de la sonde sur un AP.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuProbe {
    pub user_entries: u64,
    pub gs_failures: u64,
    pub exits: u64,
    pub exit_gs_ok: u64,
    pub fsgsbase_on: u64,
}

#[derive(Clone, Copy)]
pub struct Ring3Qualification {
    pub cpus: usize,
    pub probes: [CpuProbe; MAX_PROBED_CPUS],
    pub cr4_fsgsbase: bool,
}

/// GS utilisateur de la sonde sur `cpu` : une adresse canonique de la région
/// utilisateur, jamais mappée, distincte par CPU.
const fn user_gs(cpu: usize) -> u64 {
    ring3::USER_BASE + 0x10_0000 + cpu as u64 * PAGE_SIZE
}

fn read_msr(msr: u32) -> u64 {
    // SAFETY: IA32_GS_BASE et IA32_KERNEL_GS_BASE existent sur tout x86_64.
    unsafe { cpu::read_msr(msr) }
}

/// Appelé par les dispatchers d'IRQ, juste après le stub d'entrée. Ne lit pas
/// GS : le contrôle porte sur ce que le stub a fait de GS.
pub fn note_entry(saved_cs: u64) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let probe = PROBE_CPU.load(Ordering::Acquire) as usize;
    let gs_base = read_msr(IA32_GS_BASE);
    let expected_user = EXPECTED_USER_GS.load(Ordering::Acquire);
    if gs_base == expected_user {
        // Le noyau tourne avec le GS de la sonde : `swapgs` manquant.
        GS_FAILURES.fetch_add(1, Ordering::AcqRel);
        return;
    }
    if gs_base != cpu_local::area_address(probe) || !ring3::from_user(saved_cs) {
        // Un autre CPU, ou une entrée depuis le noyau.
        return;
    }
    if read_msr(IA32_KERNEL_GS_BASE) != expected_user {
        GS_FAILURES.fetch_add(1, Ordering::AcqRel);
    }
    if USER_ENTRIES.fetch_add(1, Ordering::AcqRel) + 1 >= TARGET_ENTRIES {
        // SAFETY: la page de pile de la sonde, mappée en identité, n'est lue
        // que par la boucle utilisateur de ce CPU.
        unsafe { write_volatile(FLAG_FRAME.load(Ordering::Acquire) as *mut u64, 1) };
    }
}

#[unsafe(no_mangle)]
extern "C" fn elyon_user_probe_exit_dispatch(saved_cs: u64) -> ! {
    let probe = PROBE_CPU.load(Ordering::Acquire) as usize;
    let ok = ring3::from_user(saved_cs)
        && read_msr(IA32_GS_BASE) == cpu_local::area_address(probe)
        && read_msr(IA32_KERNEL_GS_BASE) == EXPECTED_USER_GS.load(Ordering::Acquire);
    if ok {
        EXIT_GS_OK.fetch_add(1, Ordering::AcqRel);
    }
    EXITS.fetch_add(1, Ordering::AcqRel);
    DONE.store(true, Ordering::Release);
    // La tâche se termine sur sa pile noyau : rien n'y était plus vivant.
    sched::exit_now()
}

#[unsafe(no_mangle)]
extern "C" fn elyon_user_probe_task_entry(arg: u64) -> ! {
    if arg & !0xFFFF != PROBE_ARG_BASE {
        sched::fail_from(SchedError::ArgMismatch);
    }
    cpu::disable_interrupts();
    if cpu::read_cr4() & CR4_FSGSBASE != 0 {
        // Le ring 3 pourrait écrire sa propre base GS : politique violée.
        FSGSBASE_ON.fetch_add(1, Ordering::AcqRel);
    }
    let cpu = sched::this_cpu();
    // SAFETY: MSR architectural ; le GS utilisateur ne sert qu'au ring 3.
    unsafe { cpu::write_msr(IA32_KERNEL_GS_BASE, user_gs(cpu)) };
    // SAFETY: recharger CR3 avec sa propre valeur vide le TLB de ce CPU : la
    // région utilisateur vient d'être mappée par le BSP.
    unsafe { asm!("mov {0}, cr3", "mov cr3, {0}", out(reg) _, options(nostack, preserves_flags)) };
    let frame = match ring3::iret_frame(ring3::USER_CODE_VA, ring3::USER_STACK_TOP) {
        Ok(frame) => frame,
        Err(_) => sched::fail_from(SchedError::ArgMismatch),
    };
    // SAFETY: cadre validé par `ring3::iret_frame` ; KERNEL_GS_BASE porte le
    // GS utilisateur ; RSP0 est le sommet de la pile de cette tâche (K005-01),
    // abandonnée ici : la prochaine entrée depuis le ring 3 repart de RSP0.
    unsafe { idt::enter_user(&frame) }
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<Option<Ring3Qualification>, ProbeError> {
    cpu::disable_interrupts();
    let cpus = sched::online_cpus();
    if cpus < 2 {
        return Ok(None);
    }
    if cpus > MAX_PROBED_CPUS {
        return Err(ProbeError::Sched(SchedError::TraceOverflow));
    }
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };

    // Pages permanentes de la sonde : code (lecture seule pour le ring 3) et pile.
    let code = memory::allocate_frame().ok_or(ProbeError::AllocationFailed)?;
    let stack = memory::allocate_frame().ok_or(ProbeError::AllocationFailed)?;
    // SAFETY: le code de la sonde est une plage statique du noyau, plus petite
    // qu'une page ; la page de code vient d'être allouée et est mappée en identité.
    unsafe {
        let start = &raw const elyon_user_probe_code_start;
        let end = &raw const elyon_user_probe_code_end;
        let len = end as usize - start as usize;
        if len > PAGE_SIZE as usize {
            return Err(ProbeError::AllocationFailed);
        }
        copy_nonoverlapping(start, code as *mut u8, len);
    }
    paging::map_user_probe(code, stack)?;
    paging::check_user_probe()?;
    FLAG_FRAME.store(stack, Ordering::Release);

    let entry = elyon_user_probe_task_entry as *const () as usize as u64;
    let mut probes = [CpuProbe::default(); MAX_PROBED_CPUS];
    for (ap, probe) in probes.iter_mut().enumerate().take(cpus).skip(1) {
        for counter in [
            &USER_ENTRIES,
            &GS_FAILURES,
            &EXITS,
            &EXIT_GS_OK,
            &FSGSBASE_ON,
        ] {
            counter.store(0, Ordering::Release);
        }
        // SAFETY: page de pile de la sonde, mappée en identité ; aucune sonde ne tourne.
        unsafe { write_volatile(stack as *mut u64, 0) };
        DONE.store(false, Ordering::Release);
        PROBE_CPU.store(ap as u64, Ordering::Release);
        EXPECTED_USER_GS.store(user_gs(ap), Ordering::Release);
        ACTIVE.store(true, Ordering::Release);

        let (task_id, _region) = sched::spawn_on(
            1 << ap,
            entry,
            PROBE_ARG_BASE | ap as u64,
            bsp,
            identity_limit,
        )?;
        sched::enqueue(task_id, true)?;

        let mut rounds = 0;
        while !DONE.load(Ordering::Acquire) {
            if rounds == MAX_IPI_ROUNDS {
                ACTIVE.store(false, Ordering::Release);
                return Err(ProbeError::Timeout);
            }
            sched::send_resched(ap)?;
            timer::pit_delay_us(1_000).map_err(|_| ProbeError::Timeout)?;
            rounds += 1;
        }
        ACTIVE.store(false, Ordering::Release);

        let id = task::lookup(task_id).map_err(SchedError::Task)?;
        let mut spins = 0u64;
        while task::snapshot(id).map_err(SchedError::Task)?.state != TaskState::Dead
            || sched::reap(task_id)? == ReapOutcome::NotYet
        {
            spins += 1;
            if spins > REAP_SPINS {
                return Err(ProbeError::Sched(SchedError::OnCpuWaitTimeout));
            }
            core::hint::spin_loop();
        }

        *probe = CpuProbe {
            user_entries: USER_ENTRIES.load(Ordering::Acquire),
            gs_failures: GS_FAILURES.load(Ordering::Acquire),
            exits: EXITS.load(Ordering::Acquire),
            exit_gs_ok: EXIT_GS_OK.load(Ordering::Acquire),
            fsgsbase_on: FSGSBASE_ON.load(Ordering::Acquire),
        };
    }
    PROBE_CPU.store(u64::MAX, Ordering::Release);

    Ok(Some(Ring3Qualification {
        cpus,
        probes,
        cr4_fsgsbase: cpu::read_cr4() & CR4_FSGSBASE != 0,
    }))
}
