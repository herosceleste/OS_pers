//! K005-03 : qualification de l'entrée « paranoïaque » NMI et #MC.
//!
//! Sur chaque AP, à tour de rôle, une tâche noyau attend, IRQ masquées, trois
//! événements :
//! 1. une NMI réelle du BSP, en mode noyau avec le GS noyau ;
//! 2. une NMI réelle dans la fenêtre `swapgs`/`iretq` : CS noyau mais GS_BASE
//!    déjà utilisateur, exactement l'état de `elyon_return_to_user` ;
//! 3. `int 18` dans la même fenêtre : même porte et même pile IST qu'un #MC.
//!
//! Le BSP passe ensuite par `int 2` et `int 18` dans la même fenêtre : ses
//! piles IST NMI et #MC sont exercées aussi. Un #MC matériel, de classe
//! « abort », n'est jamais repris : seul l'`int 18` de la sonde revient.
//!
//! Le gestionnaire relève, sans passer par GS, le GS_BASE d'entrée et celui
//! qu'il voit, puis l'index lu via `gs:[0]` et son RSP ; le BSP vérifie ces
//! relevés avec `elyon_core::paranoid`. Après chaque événement, la tâche
//! vérifie que le stub a remis le GS d'entrée : sinon la fenêtre aurait repris
//! avec le GS du noyau, et `iretq` aurait livré au ring 3 la zone per-CPU.
//! Le GS utilisateur de la sonde est une adresse utilisateur jamais mappée :
//! un accès noyau via ce GS lèverait #PF au lieu de lire une valeur plausible.

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::paranoid::{self, Expected, Observation, ParanoidError};
use elyon_core::ring3;
use elyon_core::segments::LAYOUT;
use elyon_core::task_stack::StackRegion;

use crate::arch::x86_64::{apic, cpu, cpu_local, gdt, idt, percpu};
use crate::sched::{self, ReapOutcome, SchedError};
use crate::task::{self, TaskState};
use crate::timer;

const IA32_GS_BASE: u32 = 0xC000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
const IA32_MCG_STATUS: u32 = 0x17A;
/// MCG_STATUS.MCIP : un #MC matériel est en cours.
const MCG_STATUS_MCIP: u64 = 1 << 2;
/// CPUID.01H:EDX.MCA : les MSR de machine check existent.
const CPUID_EDX_MCA: u32 = 1 << 14;
const PROBE_ARG_BASE: u64 = 0xE1_0503_0000_0000;
/// Tours de `pause` au plus dans une attente (plusieurs secondes).
const WAIT_SPINS: u64 = 500_000_000;
/// Délais de 100 µs au plus, par AP, avant d'abandonner.
const MAX_POLL_ROUNDS: u32 = 50_000;
const REAP_SPINS: u64 = 50_000_000;
const MAX_PROBED_CPUS: usize = sched::MAX_SCHED_CPUS;

/// Événements de la sonde, dans l'ordre.
const NMI_KERNEL: usize = 0;
const NMI_WINDOW: usize = 1;
const MC_WINDOW: usize = 2;
/// BSP : `int 2` dans la fenêtre GS utilisateur (même porte et IST qu'une NMI).
const NMI_SOFT_WINDOW: usize = 3;
/// Événements d'un AP ; le BSP fait `NMI_SOFT_WINDOW` puis `MC_WINDOW`.
const AP_EVENTS: usize = 3;
const EVENTS: usize = 4;
const NO_EVENT: u64 = u64::MAX;

global_asm!(
    r#"
// u64 elyon_paranoid_wait(u64 *ready, const u64 *handled, u64 window, u64 spins)
// Attend que *handled devienne non nul, au plus `spins` tours ; rend les
// tours restants (0 : délai dépassé). Avec `window` non nul, l'attente a lieu
// entre deux `swapgs`, CS noyau et GS_BASE utilisateur ; *ready n'est levé
// qu'une fois dans cet état. Aucun accès via GS ni aucune pile entre les deux.
.global elyon_paranoid_wait
elyon_paranoid_wait:
    test rdx, rdx
    jz .Lparanoid_wait_armed
    swapgs
.Lparanoid_wait_armed:
    mov qword ptr [rdi], 1
.Lparanoid_wait_loop:
    cmp qword ptr [rsi], 0
    jne .Lparanoid_wait_done
    pause
    dec rcx
    jnz .Lparanoid_wait_loop
.Lparanoid_wait_done:
    test rdx, rdx
    jz .Lparanoid_wait_out
    swapgs
.Lparanoid_wait_out:
    mov rax, rcx
    ret
.global elyon_paranoid_wait_end
elyon_paranoid_wait_end:

// void elyon_paranoid_int18(void) : `int 18` dans la fenêtre GS utilisateur.
.global elyon_paranoid_int18
elyon_paranoid_int18:
    swapgs
    int 18
.global elyon_paranoid_int18_return
elyon_paranoid_int18_return:
    swapgs
    ret

// void elyon_paranoid_int2(void) : `int 2` dans la fenêtre GS utilisateur.
.global elyon_paranoid_int2
elyon_paranoid_int2:
    swapgs
    int 2
.global elyon_paranoid_int2_return
elyon_paranoid_int2_return:
    swapgs
    ret
"#
);

unsafe extern "C" {
    fn elyon_paranoid_wait(ready: *mut u64, handled: *const u64, window: u64, spins: u64) -> u64;
    fn elyon_paranoid_wait_end();
    fn elyon_paranoid_int18();
    fn elyon_paranoid_int18_return();
    fn elyon_paranoid_int2();
    fn elyon_paranoid_int2_return();
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static PROBE_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
/// Événement attendu par le gestionnaire, `NO_EVENT` hors attente.
static PHASE: AtomicU64 = AtomicU64::new(NO_EVENT);
static READY: AtomicU64 = AtomicU64::new(0);
/// Événement réclamé par le gestionnaire (une fois par attente).
static HANDLED: AtomicU64 = AtomicU64::new(0);
/// Relevés de l'événement publiés : c'est ce drapeau qu'attend la tâche.
static DISPATCHED: AtomicU64 = AtomicU64::new(0);
/// Relevés du gestionnaire par événement : GS d'entrée, GS_BASE vu, index, RSP.
static OBSERVED: [[AtomicU64; 4]; EVENTS] = [const { [const { AtomicU64::new(0) }; 4] }; EVENTS];
/// GS remis correctement après l'événement, vu par la tâche.
static RESTORED: [AtomicBool; EVENTS] = [const { AtomicBool::new(false) }; EVENTS];
static TIMED_OUT: AtomicBool = AtomicBool::new(false);
static MCE_OK: AtomicBool = AtomicBool::new(false);
static GATES_OK: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub enum ProbeError {
    Sched(SchedError),
    Paranoid(ParanoidError),
    /// L'APIC a refusé la NMI.
    Nmi,
    /// Un événement n'est pas arrivé dans le délai.
    Timeout,
}

impl ProbeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sched(error) => error.as_str(),
            Self::Paranoid(error) => error.as_str(),
            Self::Nmi => "local APIC refused the NMI IPI",
            Self::Timeout => "paranoid probe event did not arrive in time",
        }
    }
}

impl From<SchedError> for ProbeError {
    fn from(error: SchedError) -> Self {
        Self::Sched(error)
    }
}

impl From<ParanoidError> for ProbeError {
    fn from(error: ParanoidError) -> Self {
        Self::Paranoid(error)
    }
}

/// Résultat de la sonde sur un AP : `Ok` ou l'erreur de chaque événement.
#[derive(Clone, Copy)]
pub struct CpuProbe {
    pub events: [Result<(), ParanoidError>; AP_EVENTS],
    /// Événements où le stub a dû remplacer GS_BASE.
    pub replaced: u64,
    /// Événements après lesquels la tâche a retrouvé le GS d'entrée.
    pub restored: u64,
    pub mce_ok: bool,
    pub gates_ok: bool,
}

impl CpuProbe {
    const EMPTY: Self = Self {
        events: [Err(ParanoidError::EntryGs); AP_EVENTS],
        replaced: 0,
        restored: 0,
        mce_ok: false,
        gates_ok: false,
    };
}

#[derive(Clone, Copy)]
pub struct ParanoidQualification {
    pub cpus: usize,
    pub probes: [CpuProbe; MAX_PROBED_CPUS],
    /// BSP : `int 2` puis `int 18` dans la fenêtre GS utilisateur.
    pub bsp_events: [Result<(), ParanoidError>; 2],
    pub bsp_replaced: u64,
    pub bsp_restored: u64,
    /// Portes NMI, #DF et #MC relues dans l'IDT du BSP.
    pub bsp_gates_ok: bool,
    pub bsp_mce_ok: bool,
    /// Pointeurs IST relus dans les TSS dont le mot GS est la zone du CPU.
    pub slots_ok: usize,
    pub slots: usize,
}

/// GS utilisateur de la sonde sur `cpu` : canonique, de la région
/// utilisateur, jamais mappé (la sonde K005-02 n'en mappe que deux pages).
const fn user_gs(cpu: usize) -> u64 {
    ring3::USER_BASE + 0x20_0000 + cpu as u64 * PAGE_SIZE
}

fn read_msr(msr: u32) -> u64 {
    // SAFETY: IA32_GS_BASE et IA32_KERNEL_GS_BASE existent sur tout x86_64.
    unsafe { cpu::read_msr(msr) }
}

fn address(function: unsafe extern "C" fn()) -> u64 {
    function as *const () as usize as u64
}

/// Vrai si un #MC matériel est en cours : un tel #MC n'est jamais repris.
fn hardware_machine_check() -> bool {
    cpu::cpuid_1_edx() & CPUID_EDX_MCA != 0 && read_msr(IA32_MCG_STATUS) & MCG_STATUS_MCIP != 0
}

/// Appelé par `elyon_paranoid_dispatch` pour une NMI ou un #MC. Vrai si
/// l'événement est celui qu'attend la sonde, à l'endroit attendu, sur le CPU
/// sondé, et le premier de son attente : il est alors relevé et le stub
/// revient. Sinon (NMI de chien de garde, #MC matériel, second événement),
/// diagnostic et arrêt fail-closed.
///
/// Invariant de l'IST : ce chemin ne fait ni `sti`, ni `iretq`, ni faute
/// rattrapable ; une NMI imbriquée réutiliserait sinon la même pile.
pub fn on_event(vector: u64, rip: u64, entry_gs: u64) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let phase = PHASE.load(Ordering::Acquire) as usize;
    let at_expected_place = match phase {
        NMI_KERNEL | NMI_WINDOW => {
            let start = elyon_paranoid_wait as *const () as usize as u64;
            vector == u64::from(paranoid::VECTOR_NMI)
                && rip >= start
                && rip < address(elyon_paranoid_wait_end)
        }
        MC_WINDOW => {
            vector == u64::from(paranoid::VECTOR_MACHINE_CHECK)
                && rip == address(elyon_paranoid_int18_return)
                && !hardware_machine_check()
        }
        NMI_SOFT_WINDOW => {
            vector == u64::from(paranoid::VECTOR_NMI) && rip == address(elyon_paranoid_int2_return)
        }
        _ => false,
    };
    if !at_expected_place {
        return false;
    }
    let probe = PROBE_CPU.load(Ordering::Acquire) as usize;
    if probe >= MAX_PROBED_CPUS {
        return false;
    }
    let gs_base = read_msr(IA32_GS_BASE);
    // Sur le CPU sondé, GS_BASE est la zone de ce CPU (stub correct) ou le GS
    // utilisateur de la sonde (stub défaillant, relevé pour échouer) ; toute
    // autre valeur désigne un autre CPU.
    if gs_base != cpu_local::area_address(probe) && gs_base != user_gs(probe) {
        return false;
    }
    // Un seul événement par attente : un second est inattendu, donc fatal.
    if HANDLED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    // L'index n'est lu via GS que si GS_BASE est la zone de ce CPU : avec le
    // GS utilisateur, la lecture fauterait au lieu d'échouer proprement.
    let index = if gs_base == cpu_local::area_address(probe) {
        cpu_local::index() as u64
    } else {
        u64::MAX
    };
    let observed = &OBSERVED[phase];
    observed[0].store(entry_gs, Ordering::Release);
    observed[1].store(gs_base, Ordering::Release);
    observed[2].store(index, Ordering::Release);
    observed[3].store(cpu::read_rsp(), Ordering::Release);
    DISPATCHED.store(1, Ordering::Release);
    true
}

/// Après un événement : GS_BASE et KERNEL_GS_BASE doivent être revenus à
/// l'état d'avant. Corrige l'état sinon, pour que la tâche finisse proprement.
fn check_restored(event: usize, area: u64, user: u64) {
    let gs_base = read_msr(IA32_GS_BASE);
    let kernel_gs_base = read_msr(IA32_KERNEL_GS_BASE);
    let ok = gs_base == area && kernel_gs_base == user;
    if !ok {
        // SAFETY: MSR architecturaux ; remet le GS noyau avant tout accès via GS.
        unsafe {
            cpu::write_msr(IA32_GS_BASE, area);
            cpu::write_msr(IA32_KERNEL_GS_BASE, user);
        }
    }
    RESTORED[event].store(ok, Ordering::Release);
}

fn arm(event: usize) {
    HANDLED.store(0, Ordering::Release);
    DISPATCHED.store(0, Ordering::Release);
    PHASE.store(event as u64, Ordering::Release);
}

fn wait_for(event: usize, window: bool) -> bool {
    // READY est remis à zéro avant la nouvelle phase : le BSP ne peut pas
    // associer le READY de l'attente précédente à celle-ci.
    READY.store(0, Ordering::Release);
    arm(event);
    // SAFETY: les deux drapeaux sont des statiques ; `elyon_paranoid_wait`
    // n'utilise ni GS ni la pile entre ses deux `swapgs`.
    let remaining = unsafe {
        elyon_paranoid_wait(
            READY.as_ptr(),
            DISPATCHED.as_ptr(),
            u64::from(window),
            WAIT_SPINS,
        )
    };
    remaining != 0 || DISPATCHED.load(Ordering::Acquire) != 0
}

/// `int 2` ou `int 18` dans la fenêtre GS utilisateur ; vrai si le
/// gestionnaire l'a relevé.
fn soft_event(event: usize) -> bool {
    arm(event);
    // SAFETY: `int 2`/`int 18` vers une porte DPL 0 à IST dédiée, depuis le
    // ring 0, entre deux `swapgs` ; la sonde l'attend, le stub revient et
    // remet le GS utilisateur avant le second `swapgs`.
    unsafe {
        if event == MC_WINDOW {
            elyon_paranoid_int18();
        } else {
            elyon_paranoid_int2();
        }
    }
    DISPATCHED.load(Ordering::Acquire) != 0
}

fn gates_ok() -> bool {
    [
        paranoid::VECTOR_NMI,
        paranoid::VECTOR_DOUBLE_FAULT,
        paranoid::VECTOR_MACHINE_CHECK,
    ]
    .iter()
    .all(|&vector| {
        let Some(ist) = paranoid::ist_for_vector(vector) else {
            return false;
        };
        idt::loaded_gate(vector).is_some_and(|(low, high)| {
            paranoid::check_gate(
                low,
                high,
                LAYOUT.kernel_code,
                idt::paranoid_handler(vector),
                ist,
            )
            .is_ok()
        })
    })
}

#[unsafe(no_mangle)]
extern "C" fn elyon_paranoid_probe_task_entry(arg: u64) -> ! {
    if arg & !0xFFFF != PROBE_ARG_BASE {
        sched::fail_from(SchedError::ArgMismatch);
    }
    cpu::disable_interrupts();
    MCE_OK.store(
        paranoid::check_mce(cpu::read_cr4(), cpu::cpuid_1_edx()).is_ok(),
        Ordering::Release,
    );
    GATES_OK.store(gates_ok(), Ordering::Release);
    let cpu = sched::this_cpu();
    let area = cpu_local::area_address(cpu);
    let user = user_gs(cpu);
    // SAFETY: MSR architectural ; le GS utilisateur ne sert qu'à la fenêtre.
    unsafe { cpu::write_msr(IA32_KERNEL_GS_BASE, user) };

    let mut timed_out = false;
    for (event, window) in [(NMI_KERNEL, false), (NMI_WINDOW, true)] {
        timed_out |= !wait_for(event, window);
        check_restored(event, area, user);
    }
    timed_out |= !soft_event(MC_WINDOW);
    check_restored(MC_WINDOW, area, user);

    PHASE.store(NO_EVENT, Ordering::Release);
    TIMED_OUT.store(timed_out, Ordering::Release);
    // Le GS de la sonde ne lui survit pas.
    // SAFETY: MSR architectural ; GS_BASE est la zone de ce CPU.
    unsafe { cpu::write_msr(IA32_KERNEL_GS_BASE, 0) };
    DONE.store(true, Ordering::Release);
    sched::exit_now()
}

fn observation(event: usize) -> Observation {
    let observed = &OBSERVED[event];
    Observation {
        entry_gs: observed[0].load(Ordering::Acquire),
        dispatch_gs: observed[1].load(Ordering::Acquire),
        dispatch_index: observed[2].load(Ordering::Acquire),
        rsp: observed[3].load(Ordering::Acquire),
    }
}

/// Pointeurs IST des TSS de tous les CPU : ceux de `paranoid::ist_pointer`,
/// chacun sous le mot GS de son CPU ; piles deux à deux disjointes.
fn check_slots(cpus: usize) -> Result<usize, ParanoidError> {
    let mut spans = [(0, 0); MAX_PROBED_CPUS * gdt::PARANOID_ISTS];
    let mut slots_ok = 0;
    for cpu in 0..cpus {
        for index in 0..gdt::PARANOID_ISTS as u16 {
            let (start, end) = gdt::ist_span_for(cpu, index);
            spans[cpu * gdt::PARANOID_ISTS + usize::from(index)] = (start, end);
            let (pointer, slot) = gdt::ist_pointer_and_slot(cpu, index);
            if Ok(pointer) == paranoid::ist_pointer(start, end)
                && slot == cpu_local::area_address(cpu)
            {
                slots_ok += 1;
            }
        }
    }
    paranoid::check_spans(&spans[..cpus * gdt::PARANOID_ISTS])?;
    Ok(slots_ok)
}

fn reset_observations() {
    for observed in OBSERVED.iter().flatten() {
        observed.store(0, Ordering::Release);
    }
    for restored in &RESTORED {
        restored.store(false, Ordering::Release);
    }
}

/// Résultat d'un événement de `cpu` contre ce que la qualification attend.
fn check_event(cpu: usize, event: usize, entry_gs: u64) -> Result<(), ParanoidError> {
    let ist_index = if event == MC_WINDOW {
        gdt::MACHINE_CHECK_IST_INDEX
    } else {
        gdt::NMI_IST_INDEX
    };
    paranoid::check_observation(
        &observation(event),
        &Expected {
            kernel_gs: cpu_local::area_address(cpu),
            entry_gs,
            index: cpu as u64,
            ist: gdt::ist_span_for(cpu, ist_index),
        },
    )
}

/// BSP : `int 2` puis `int 18` dans la fenêtre GS utilisateur, IRQ masquées.
type BspProbe = ([Result<(), ParanoidError>; 2], u64, u64);

fn probe_bsp() -> Result<BspProbe, ProbeError> {
    let area = cpu_local::area_address(0);
    let user = user_gs(0);
    reset_observations();
    PROBE_CPU.store(0, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
    // SAFETY: MSR architectural ; le GS utilisateur ne sert qu'à la fenêtre.
    unsafe { cpu::write_msr(IA32_KERNEL_GS_BASE, user) };
    let mut handled = true;
    for event in [NMI_SOFT_WINDOW, MC_WINDOW] {
        handled &= soft_event(event);
        check_restored(event, area, user);
    }
    PHASE.store(NO_EVENT, Ordering::Release);
    ACTIVE.store(false, Ordering::Release);
    // SAFETY: MSR architectural ; GS_BASE est la zone du BSP.
    unsafe { cpu::write_msr(IA32_KERNEL_GS_BASE, 0) };
    if !handled {
        return Err(ProbeError::Timeout);
    }
    let mut replaced = 0;
    let mut restored = 0;
    let events = [NMI_SOFT_WINDOW, MC_WINDOW].map(|event| {
        replaced += u64::from(paranoid::replaced(&observation(event), area));
        restored += u64::from(RESTORED[event].load(Ordering::Acquire));
        check_event(0, event, user)
    });
    Ok((events, replaced, restored))
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<Option<ParanoidQualification>, ProbeError> {
    cpu::disable_interrupts();
    let cpus = sched::online_cpus();
    if cpus < 2 {
        return Ok(None);
    }
    if cpus > MAX_PROBED_CPUS {
        return Err(ProbeError::Sched(SchedError::TraceOverflow));
    }
    let slots_ok = check_slots(cpus)?;
    let (bsp_events, bsp_replaced, bsp_restored) = probe_bsp()?;
    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };

    let entry = elyon_paranoid_probe_task_entry as *const () as usize as u64;
    let mut probes = [CpuProbe::EMPTY; MAX_PROBED_CPUS];
    for (ap, probe) in probes.iter_mut().enumerate().take(cpus).skip(1) {
        let apic_id = percpu::snapshot(ap)
            .ok_or(ProbeError::Sched(SchedError::ApTimeout))?
            .apic_id;
        reset_observations();
        for flag in [&TIMED_OUT, &MCE_OK, &GATES_OK, &DONE] {
            flag.store(false, Ordering::Release);
        }
        PHASE.store(NO_EVENT, Ordering::Release);
        PROBE_CPU.store(ap as u64, Ordering::Release);
        ACTIVE.store(true, Ordering::Release);

        let spawned = sched::spawn_on(
            1 << ap,
            entry,
            PROBE_ARG_BASE | ap as u64,
            bsp,
            identity_limit,
        )
        .and_then(|(task_id, _region)| sched::enqueue(task_id, true).map(|_| task_id));
        let task_id = match spawned {
            Ok(task_id) => task_id,
            Err(error) => {
                ACTIVE.store(false, Ordering::Release);
                return Err(ProbeError::Sched(error));
            }
        };

        // Une NMI par attente armée : la tâche lève READY une fois dans
        // l'état voulu (GS noyau, puis fenêtre GS utilisateur).
        let mut sent_for = NO_EVENT;
        let mut rounds = 0;
        while !DONE.load(Ordering::Acquire) {
            let phase = PHASE.load(Ordering::Acquire);
            if phase <= NMI_WINDOW as u64 && phase != sent_for && READY.load(Ordering::Acquire) != 0
            {
                if apic::send_nmi(apic_id).is_err() {
                    ACTIVE.store(false, Ordering::Release);
                    return Err(ProbeError::Nmi);
                }
                sent_for = phase;
            }
            if rounds == MAX_POLL_ROUNDS {
                ACTIVE.store(false, Ordering::Release);
                return Err(ProbeError::Timeout);
            }
            timer::pit_delay_us(100).map_err(|_| ProbeError::Timeout)?;
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
        if TIMED_OUT.load(Ordering::Acquire) {
            return Err(ProbeError::Timeout);
        }

        let area = cpu_local::area_address(ap);
        let mut result = CpuProbe::EMPTY;
        for (event, outcome) in result.events.iter_mut().enumerate() {
            let entry_gs = if event == NMI_KERNEL {
                area
            } else {
                user_gs(ap)
            };
            *outcome = check_event(ap, event, entry_gs);
            result.replaced += u64::from(paranoid::replaced(&observation(event), area));
            result.restored += u64::from(RESTORED[event].load(Ordering::Acquire));
        }
        result.mce_ok = MCE_OK.load(Ordering::Acquire);
        result.gates_ok = GATES_OK.load(Ordering::Acquire);
        *probe = result;
    }
    PROBE_CPU.store(u64::MAX, Ordering::Release);

    Ok(Some(ParanoidQualification {
        cpus,
        probes,
        bsp_events,
        bsp_replaced,
        bsp_restored,
        bsp_gates_ok: gates_ok(),
        bsp_mce_ok: paranoid::check_mce(cpu::read_cr4(), cpu::cpuid_1_edx()).is_ok(),
        slots_ok,
        slots: cpus * gdt::PARANOID_ISTS,
    }))
}
