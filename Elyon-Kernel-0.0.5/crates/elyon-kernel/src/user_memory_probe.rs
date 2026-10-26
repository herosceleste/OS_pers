//! K005-05 : qualification des pages utilisateur.
//!
//! Sur le BSP : les quatre pages permanentes des sondes K005-02 et K005-04
//! viennent de `allocate_owned`, portent des droits exacts (code lecture
//! seule exécutable, données NX) et n'ont plus d'alias identité ; le mappage
//! identité ne porte U/S nulle part.
//!
//! Sur chaque CPU, le BSP d'abord puis chaque AP dans une tâche :
//! 1. EFER.NXE actif ;
//! 2. une lecture de chaque page permanente par son alias identité lève une
//!    #PF (page absente, lecture, superviseur) reprise par la sonde ;
//! 3. cycle de vie d'une page : la frame, prise par `allocate_owned`, est lue
//!    par son alias (traduction en TLB) ; le BSP en fait une `UserPage`, et
//!    l'alias doit alors lever une #PF : sur un AP, seule l'IPI de shootdown
//!    a pu retirer la traduction ; la page se lit par la copie validée ; le
//!    BSP la libère, et la copie doit échouer (shootdown de l'adresse
//!    utilisateur) ; l'alias revient, avec le contenu de la page.
//!
//! Les AP attendent entre deux étapes IRQ actives, pour recevoir l'IPI ;
//! chaque étape tourne IRQ masquées.

use core::arch::global_asm;
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::ring3::{self, Ring3Error};
use elyon_core::task_stack::StackRegion;
use elyon_core::user_access::{self, FaultSite, UserAccessError};
use elyon_core::user_pages::{self, PageKind};

use crate::arch::x86_64::{cpu, cpu_local, paging};
use crate::sched::{self, ReapOutcome, SchedError};
use crate::task::{self, TaskState};
use crate::user_memory::{UserMemoryError, UserPage};
use crate::{memory, timer, tlb, user_access_probe, user_copy, user_probe};

const PROBE_ARG_BASE: u64 = 0xE1_0505_0000_0000;
const MAX_POLL_ROUNDS: u32 = 50_000;
const REAP_SPINS: u64 = 50_000_000;
/// Tours d'attente d'une étape par l'AP, IRQ actives.
const STEP_SPINS: u64 = 500_000_000;
const MAX_PROBED_CPUS: usize = sched::MAX_SCHED_CPUS;
/// Page du cycle de vie, libre dans la table utilisateur.
const LIFECYCLE_VA: u64 = ring3::USER_BASE + 7 * PAGE_SIZE;
/// Valeur rendue par la sonde quand la lecture par l'alias a pris la faute.
const ALIAS_MARK: u64 = 0xA11A_5A11_A55A_11A5;
/// Code d'erreur d'une lecture superviseur d'une page absente.
const ALIAS_ERROR: u64 = 0;
/// Pages permanentes des sondes K005-02 et K005-04.
const PERMANENT_PAGES: usize = 4;

global_asm!(
    r#"
// u64 elyon_alias_probe(const u64 *alias)
// Lecture par le mappage identité. Rend la valeur lue, ou la marque si la
// lecture a pris une #PF (reprise en elyon_alias_probe_resume).
.global elyon_alias_probe
elyon_alias_probe:
    movabs rax, {mark}
.global elyon_alias_probe_access
elyon_alias_probe_access:
    mov rax, qword ptr [rdi]
.global elyon_alias_probe_resume
elyon_alias_probe_resume:
    ret
"#,
    mark = const ALIAS_MARK,
);

unsafe extern "C" {
    fn elyon_alias_probe(alias: *const u64) -> u64;
    fn elyon_alias_probe_access();
    fn elyon_alias_probe_resume();
}

static ARMED: AtomicBool = AtomicBool::new(false);
static PROBE_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
static EXPECTED_CR2: AtomicU64 = AtomicU64::new(0);
static FIRED: AtomicBool = AtomicBool::new(false);
static FAULT_ERROR: AtomicU64 = AtomicU64::new(u64::MAX);

/// Étapes du cycle de vie, publiées par le BSP.
static STEP: AtomicU64 = AtomicU64::new(0);
/// Dernière étape terminée par l'AP.
static AP_STEP: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE_FRAME: AtomicU64 = AtomicU64::new(0);
static RESULTS: [AtomicU64; MAX_PROBED_CPUS * FIELDS] =
    [const { AtomicU64::new(0) }; MAX_PROBED_CPUS * FIELDS];

const F_NXE: usize = 0;
const F_ALIAS_FAULTS: usize = 1;
const F_WARM: usize = 2;
const F_ALIAS_SHOT: usize = 3;
const F_USER: usize = 4;
const F_USER_SHOT: usize = 5;
const F_ALIAS_BACK: usize = 6;
const F_IPIS: usize = 7;
const FIELDS: usize = 8;

const STEP_WARM: u64 = 1;
const STEP_MAPPED: u64 = 2;
const STEP_RELEASED: u64 = 3;

#[derive(Clone, Copy)]
pub enum ProbeError {
    Sched(SchedError),
    Ring3(Ring3Error),
    Memory(UserMemoryError),
    AllocationFailed,
    Timeout,
}

impl ProbeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sched(error) => error.as_str(),
            Self::Ring3(error) => error.as_str(),
            Self::Memory(error) => error.as_str(),
            Self::AllocationFailed => "PMM could not allocate the lifecycle page",
            Self::Timeout => "user page probe did not finish in time",
        }
    }
}

impl From<SchedError> for ProbeError {
    fn from(error: SchedError) -> Self {
        Self::Sched(error)
    }
}

impl From<UserMemoryError> for ProbeError {
    fn from(error: UserMemoryError) -> Self {
        Self::Memory(error)
    }
}

/// Résultat de la sonde sur un CPU.
#[derive(Clone, Copy, Default)]
pub struct CpuPages {
    pub nxe: bool,
    /// Pages permanentes dont l'alias a levé la #PF attendue.
    pub alias_faults: u64,
    /// Alias de la frame lu avant la création de la page.
    pub warm: bool,
    /// Alias en faute après la création de la page.
    pub alias_shot: bool,
    /// Page lue par la copie validée.
    pub user: bool,
    /// Copie en échec après la libération.
    pub user_shot: bool,
    /// Alias revenu après la libération, contenu intact.
    pub alias_back: bool,
    /// IPI de shootdown reçues pendant le cycle (0 sur le BSP).
    pub ipis: u64,
}

impl CpuPages {
    pub const fn ok(&self, bsp: bool) -> bool {
        self.nxe
            && self.alias_faults == PERMANENT_PAGES as u64
            && self.warm
            && self.alias_shot
            && self.user
            && self.user_shot
            && self.alias_back
            && (bsp || self.ipis >= 2)
    }

    fn store(&self, cpu: usize) {
        let values = [
            u64::from(self.nxe),
            self.alias_faults,
            u64::from(self.warm),
            u64::from(self.alias_shot),
            u64::from(self.user),
            u64::from(self.user_shot),
            u64::from(self.alias_back),
            self.ipis,
        ];
        for (field, value) in values.into_iter().enumerate() {
            RESULTS[cpu * FIELDS + field].store(value, Ordering::Release);
        }
    }

    fn load(cpu: usize) -> Self {
        let field = |index: usize| RESULTS[cpu * FIELDS + index].load(Ordering::Acquire);
        Self {
            nxe: field(F_NXE) != 0,
            alias_faults: field(F_ALIAS_FAULTS),
            warm: field(F_WARM) != 0,
            alias_shot: field(F_ALIAS_SHOT) != 0,
            user: field(F_USER) != 0,
            user_shot: field(F_USER_SHOT) != 0,
            alias_back: field(F_ALIAS_BACK) != 0,
            ipis: field(F_IPIS),
        }
    }
}

#[derive(Clone, Copy)]
pub struct UserPagesQualification {
    pub cpus: usize,
    pub probes: [CpuPages; MAX_PROBED_CPUS],
    /// Pages permanentes aux droits exacts, sans alias, prises par
    /// `allocate_owned`.
    pub user_pages: usize,
    pub owned: usize,
    /// Tables de découpe du mappage identité, qui ne porte U/S nulle part.
    pub split_tables: usize,
    /// Frames libres du PMM revenues à leur niveau après les cycles, tables
    /// de découpe neuves déduites.
    pub pmm_balanced: bool,
}

fn address(function: unsafe extern "C" fn()) -> u64 {
    function as *const () as usize as u64
}

/// Appelé par le dispatcher d'exceptions pour une #PF : reprend la lecture
/// par l'alias armée sur ce CPU, une seule fois.
pub fn on_fault(site: &FaultSite) -> Option<u64> {
    if ring3::from_user(site.cs)
        || site.vector != user_access::VECTOR_PAGE_FAULT
        || site.rip != address(elyon_alias_probe_access)
        || site.cr2 != EXPECTED_CR2.load(Ordering::Acquire)
        || PROBE_CPU.load(Ordering::Acquire) != cpu_local::index() as u64
    {
        return None;
    }
    if ARMED
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    FAULT_ERROR.store(site.error_code, Ordering::Release);
    FIRED.store(true, Ordering::Release);
    Some(address(elyon_alias_probe_resume))
}

/// Vrai si la lecture de `frame` par son alias identité lève la #PF d'une
/// page absente.
fn alias_faults(cpu: usize, frame: u64) -> bool {
    FIRED.store(false, Ordering::Release);
    FAULT_ERROR.store(u64::MAX, Ordering::Release);
    EXPECTED_CR2.store(frame, Ordering::Release);
    PROBE_CPU.store(cpu as u64, Ordering::Release);
    ARMED.store(true, Ordering::Release);
    // SAFETY: lecture d'une adresse du mappage identité ; absente, elle lève
    // une #PF reprise par `on_fault`, qui rend la marque.
    let value = unsafe { elyon_alias_probe(frame as *const u64) };
    ARMED.store(false, Ordering::Release);
    FIRED.load(Ordering::Acquire)
        && value == ALIAS_MARK
        && FAULT_ERROR.load(Ordering::Acquire) == ALIAS_ERROR
}

fn pattern(cpu: usize) -> u64 {
    0x5EED_0000_0000_0505 | (cpu as u64) << 16
}

fn permanent_pages() -> [(u64, u64, PageKind); PERMANENT_PAGES] {
    let [a, b] = user_probe::pages();
    let [c, d] = user_access_probe::pages();
    [a, b, c, d]
}

fn count_alias_faults(cpu: usize) -> u64 {
    permanent_pages()
        .iter()
        .filter(|&&(_, frame, _)| frame != 0 && alias_faults(cpu, frame))
        .count() as u64
}

/// Lecture de la page du cycle de vie par la copie validée.
fn read_lifecycle() -> Result<u64, UserAccessError> {
    let mut bytes = [0u8; 8];
    user_copy::copy_from_user(&mut bytes, LIFECYCLE_VA)?;
    Ok(u64::from_ne_bytes(bytes))
}

/// Étape de l'AP : le BSP l'a publiée ; l'AP la traite, puis l'acquitte.
fn ap_step(step: u64, result: &mut CpuPages, cpu: usize) {
    let frame = LIFECYCLE_FRAME.load(Ordering::Acquire);
    match step {
        STEP_WARM => {
            // SAFETY: frame prise par le BSP, encore atteinte par son alias
            // identité ; la lecture place la traduction dans le TLB.
            result.warm = unsafe { read_volatile(frame as *const u64) } == pattern(cpu);
        }
        STEP_MAPPED => {
            result.alias_shot = alias_faults(cpu, frame);
            result.user = read_lifecycle() == Ok(pattern(cpu));
        }
        STEP_RELEASED => {
            result.user_shot = read_lifecycle() == Err(UserAccessError::Fault);
            // SAFETY: frame rendue au PMM avec son alias, non réallouée
            // pendant la sonde (le BSP attend cette étape).
            result.alias_back = unsafe { read_volatile(frame as *const u64) } == pattern(cpu);
        }
        _ => {}
    }
    AP_STEP.store(step, Ordering::Release);
}

#[unsafe(no_mangle)]
extern "C" fn elyon_user_memory_probe_task_entry(arg: u64) -> ! {
    if arg & !0xFFFF != PROBE_ARG_BASE {
        sched::fail_from(SchedError::ArgMismatch);
    }
    cpu::disable_interrupts();
    let cpu = sched::this_cpu();
    let ipis_before = tlb::received(cpu);
    let mut result = CpuPages {
        nxe: user_pages::check_efer(cpu::read_efer()).is_ok(),
        alias_faults: count_alias_faults(cpu),
        ..CpuPages::default()
    };
    let mut done = 0;
    let mut spins = 0u64;
    // IRQ actives pendant toute l'attente : l'IPI de shootdown est servie
    // ici. Chaque étape tourne IRQ masquées.
    cpu::enable_interrupts();
    while done < STEP_RELEASED && spins < STEP_SPINS {
        let step = STEP.load(Ordering::Acquire);
        if step > done {
            cpu::disable_interrupts();
            ap_step(step, &mut result, cpu);
            cpu::enable_interrupts();
            done = step;
            continue;
        }
        core::hint::spin_loop();
        spins += 1;
    }
    cpu::disable_interrupts();
    result.ipis = tlb::received(cpu) - ipis_before;
    result.store(cpu);
    AP_STEP.store(u64::MAX, Ordering::Release);
    sched::exit_now()
}

/// Publie `step` et attend que l'AP l'ait traitée.
fn bsp_step(step: u64) -> Result<(), ProbeError> {
    STEP.store(step, Ordering::Release);
    let mut rounds = 0;
    while AP_STEP.load(Ordering::Acquire) < step {
        if rounds == MAX_POLL_ROUNDS {
            return Err(ProbeError::Timeout);
        }
        timer::pit_delay_us(100).map_err(|_| ProbeError::Timeout)?;
        rounds += 1;
    }
    Ok(())
}

/// Prend une frame par `allocate_owned`, y écrit le motif de `cpu` par son
/// alias identité et la publie.
fn lifecycle_frame(cpu: usize) -> Result<memory::FrameRange, ProbeError> {
    let range = memory::allocate_owned(1).ok_or(ProbeError::AllocationFailed)?;
    // SAFETY: frame neuve, atteinte par son seul alias identité.
    unsafe { core::ptr::write_volatile(range.start() as *mut u64, pattern(cpu)) };
    LIFECYCLE_FRAME.store(range.start(), Ordering::Release);
    Ok(range)
}

fn lifecycle_page(range: memory::FrameRange, cpu: usize) -> Result<UserPage, ProbeError> {
    Ok(UserPage::from_range(
        range,
        LIFECYCLE_VA,
        PageKind::Data,
        &pattern(cpu).to_ne_bytes(),
    )?)
}

fn probe_bsp() -> Result<CpuPages, ProbeError> {
    let mut result = CpuPages {
        nxe: user_pages::check_efer(cpu::read_efer()).is_ok(),
        alias_faults: count_alias_faults(0),
        ..CpuPages::default()
    };
    let range = lifecycle_frame(0)?;
    let frame = range.start();
    // SAFETY: frame neuve, encore atteinte par son alias.
    result.warm = unsafe { read_volatile(frame as *const u64) } == pattern(0);
    let page = lifecycle_page(range, 0)?;
    result.alias_shot = alias_faults(0, frame);
    result.user = read_lifecycle() == Ok(pattern(0));
    page.release()?;
    result.user_shot = read_lifecycle() == Err(UserAccessError::Fault);
    // SAFETY: frame rendue avec son alias, non réallouée entre-temps.
    result.alias_back = unsafe { read_volatile(frame as *const u64) } == pattern(0);
    Ok(result)
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<Option<UserPagesQualification>, ProbeError> {
    cpu::disable_interrupts();
    let cpus = sched::online_cpus();
    // Les pages permanentes viennent des sondes K005-02 et K005-04, qui ne
    // tournent qu'avec au moins un AP.
    if cpus < 2 {
        return Ok(None);
    }
    if cpus > MAX_PROBED_CPUS {
        return Err(ProbeError::Sched(SchedError::TraceOverflow));
    }

    let pages = permanent_pages();
    let user_pages = pages
        .iter()
        .filter(|&&(va, frame, kind)| {
            frame != 0
                && paging::check_user_page(va, frame, kind).is_ok()
                && paging::identity_entry(frame) == Ok(0)
        })
        .count();
    let owned = pages
        .iter()
        .filter(|&&(_, frame, _)| frame != 0 && memory::is_owned(frame))
        .count();

    let splits_before = paging::check_kernel_identity().map_err(ProbeError::Ring3)?;
    let free_before = memory::free_frame_count();

    let mut probes = [CpuPages::default(); MAX_PROBED_CPUS];
    probes[0] = probe_bsp()?;

    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let entry = elyon_user_memory_probe_task_entry as *const () as usize as u64;
    for (ap, probe) in probes.iter_mut().enumerate().take(cpus).skip(1) {
        STEP.store(0, Ordering::Release);
        AP_STEP.store(0, Ordering::Release);
        let range = lifecycle_frame(ap)?;
        let (task_id, _region) = sched::spawn_on(
            1 << ap,
            entry,
            PROBE_ARG_BASE | ap as u64,
            bsp,
            identity_limit,
        )?;
        sched::enqueue(task_id, true)?;

        bsp_step(STEP_WARM)?;
        let page = lifecycle_page(range, ap)?;
        bsp_step(STEP_MAPPED)?;
        page.release()?;
        bsp_step(STEP_RELEASED)?;

        let mut rounds = 0;
        while AP_STEP.load(Ordering::Acquire) != u64::MAX {
            if rounds == MAX_POLL_ROUNDS {
                return Err(ProbeError::Timeout);
            }
            timer::pit_delay_us(100).map_err(|_| ProbeError::Timeout)?;
            rounds += 1;
        }
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
        *probe = CpuPages::load(ap);
    }
    PROBE_CPU.store(u64::MAX, Ordering::Release);

    let split_tables = paging::check_kernel_identity().map_err(ProbeError::Ring3)?;
    let new_tables = split_tables.saturating_sub(splits_before);
    // Chaque table de découpe neuve est une frame permanente du PMM.
    let pmm_balanced = memory::free_frame_count() + new_tables == free_before;

    Ok(Some(UserPagesQualification {
        cpus,
        probes,
        user_pages,
        owned,
        split_tables,
        pmm_balanced,
    }))
}
