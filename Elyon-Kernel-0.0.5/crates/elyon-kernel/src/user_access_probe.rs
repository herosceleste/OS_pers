//! K005-04 : qualification de SMEP, de SMAP et des copies utilisateur.
//!
//! Le BSP mappe trois pages dans la région utilisateur : une page de code en
//! lecture seule (`mov eax, 2 ; ret`), une page de données et une page jamais
//! mappée. Sur chaque CPU, le BSP d'abord puis chaque AP dans une tâche, IRQ
//! masquées :
//! 1. CR4 porte SMEP, SMAP et UMIP (si le CPU le gère), CR0 porte WP ;
//! 2. une lecture directe de la page de données, sans `stac`, lève une #PF de
//!    signature SMAP, reprise par la sonde ;
//! 3. un appel direct de la page de code lève une #PF de signature SMEP ;
//!    sans SMEP, la page s'exécuterait et rendrait 2 ;
//! 4. `copy_to_user` puis `copy_from_user` font l'aller-retour d'un motif
//!    propre au CPU, relu aussi par le mappage identité ;
//! 5. une écriture sur la page de code, une lecture de la page jamais mappée
//!    et une lecture qui déborde sur elle échouent par une faute reprise, sans
//!    arrêter le système ; la dernière copie les octets d'avant la faute ;
//! 6. cinq plages invalides sont refusées sans aucun accès ;
//! 7. RFLAGS.AC est nul après tout cela, et dans le gestionnaire des fautes
//!    de copie, prises pourtant AC levé.

use core::arch::{asm, global_asm};
use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use elyon_abi::PAGE_SIZE;
use elyon_core::ring3::{self, Ring3Error};
use elyon_core::task_stack::StackRegion;
use elyon_core::user_access::{
    self, FaultSite, Features, PROBE_CODE_VA, PROBE_DATA_VA, PROBE_UNMAPPED_VA, UserAccessError,
};

use crate::arch::x86_64::{cpu, cpu_local, paging};
use crate::sched::{self, ReapOutcome, SchedError};
use crate::task::{self, TaskState};
use crate::{memory, timer, user_copy};

const PROBE_ARG_BASE: u64 = 0xE1_0504_0000_0000;
/// Délais de 100 µs au plus, par AP, avant d'abandonner.
const MAX_POLL_ROUNDS: u32 = 50_000;
const REAP_SPINS: u64 = 50_000_000;
const MAX_PROBED_CPUS: usize = sched::MAX_SCHED_CPUS;
/// `mov eax, 2 ; ret` : la page de code exécutée par le noyau rendrait 2.
const USER_CODE: [u8; 6] = [0xB8, 0x02, 0x00, 0x00, 0x00, 0xC3];
/// Valeur que la sonde SMAP rend quand la lecture a pris la faute.
const SMAP_MARK: u64 = 0x5AFE_5AFE_5AFE_5AFE;
/// Premier mot de la page de données.
const DATA_WORD: u64 = 0x0DA7_A000_0000_0504;
const PATTERN_LEN: usize = 64;
/// Octets en fin de page de données, avant la page jamais mappée.
const TAIL: [u8; 8] = [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18];
/// Plages invalides essayées sur chaque CPU.
const REJECTS: u64 = 5;
/// Fautes de copie attendues sur chaque CPU : page en lecture seule, page
/// jamais mappée, lecture qui déborde.
const COPY_FAULTS: u64 = 3;

global_asm!(
    r#"
// u64 elyon_smap_probe(const u64 *user)
// Lecture directe d'une page utilisateur, sans `stac`. Rend la valeur lue, ou
// la marque si la lecture a pris une #PF (reprise en elyon_smap_probe_resume).
.global elyon_smap_probe
elyon_smap_probe:
    movabs rax, {mark}
.global elyon_smap_probe_access
elyon_smap_probe_access:
    mov rax, qword ptr [rdi]
.global elyon_smap_probe_resume
elyon_smap_probe_resume:
    ret

// u64 elyon_smep_probe(u64 target)
// Appel direct d'une page utilisateur. Rend 0 si l'exécution a pris une #PF
// (reprise en elyon_smep_probe_return, adresse de retour dépilée), sinon ce
// que la page a rendu.
.global elyon_smep_probe
elyon_smep_probe:
    xor eax, eax
    call rdi
.global elyon_smep_probe_return
elyon_smep_probe_return:
    ret
"#,
    mark = const SMAP_MARK,
);

unsafe extern "C" {
    fn elyon_smap_probe(user: *const u64) -> u64;
    fn elyon_smap_probe_access();
    fn elyon_smap_probe_resume();
    fn elyon_smep_probe(target: u64) -> u64;
    fn elyon_smep_probe_return();
}

const ARMED_NONE: u64 = 0;
const ARMED_SMAP: u64 = 1;
const ARMED_SMEP: u64 = 2;

static ARMED: AtomicU64 = AtomicU64::new(ARMED_NONE);
static PROBE_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
/// Faute relevée par `on_fault` : vecteur, CS, RIP, code d'erreur, CR2, RFLAGS.
static SITE: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static FIRED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static RESULTS: [AtomicU64; MAX_PROBED_CPUS * FIELDS] =
    [const { AtomicU64::new(0) }; MAX_PROBED_CPUS * FIELDS];

/// Champs d'un résultat dans `RESULTS`.
const F_CR4: usize = 0;
const F_SMAP: usize = 1;
const F_SMEP: usize = 2;
const F_COPY: usize = 3;
const F_READ_ONLY: usize = 4;
const F_UNMAPPED: usize = 5;
const F_PARTIAL: usize = 6;
const F_FAULTS: usize = 7;
const F_REJECTS: usize = 8;
const F_HANDLER_AC: usize = 9;
const F_AC_CLEAR: usize = 10;
const F_SMAP_ERROR: usize = 11;
const F_SMEP_ERROR: usize = 12;
const F_CR0_WP: usize = 13;
const FIELDS: usize = 14;

#[derive(Clone, Copy)]
pub enum ProbeError {
    Sched(SchedError),
    Ring3(Ring3Error),
    AllocationFailed,
    Timeout,
}

impl ProbeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sched(error) => error.as_str(),
            Self::Ring3(error) => error.as_str(),
            Self::AllocationFailed => "PMM could not allocate the user access probe pages",
            Self::Timeout => "user access probe did not finish in time",
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

/// Résultat de la sonde sur un CPU.
#[derive(Clone, Copy, Default)]
pub struct CpuAccess {
    pub cr4_ok: bool,
    pub cr0_wp: bool,
    pub smap_ok: bool,
    pub smep_ok: bool,
    pub copy_ok: bool,
    pub read_only_refused: bool,
    pub unmapped_refused: bool,
    pub partial_ok: bool,
    /// Fautes de copie reprises pendant la sonde.
    pub faults: u64,
    /// Plages invalides refusées avec l'erreur attendue et sans accès.
    pub rejects: u64,
    /// Fautes de copie dont le gestionnaire voyait RFLAGS.AC levé.
    pub handler_ac: u64,
    pub ac_clear: bool,
    /// Code d'erreur de la #PF relevée par les sondes SMAP et SMEP.
    pub smap_error: u64,
    pub smep_error: u64,
}

impl CpuAccess {
    pub const fn ok(&self) -> bool {
        self.cr4_ok
            && self.cr0_wp
            && self.smap_ok
            && self.smep_ok
            && self.copy_ok
            && self.read_only_refused
            && self.unmapped_refused
            && self.partial_ok
            && self.faults == COPY_FAULTS
            && self.rejects == REJECTS
            && self.handler_ac == 0
            && self.ac_clear
    }

    fn store(&self, cpu: usize) {
        let values = [
            u64::from(self.cr4_ok),
            u64::from(self.smap_ok),
            u64::from(self.smep_ok),
            u64::from(self.copy_ok),
            u64::from(self.read_only_refused),
            u64::from(self.unmapped_refused),
            u64::from(self.partial_ok),
            self.faults,
            self.rejects,
            self.handler_ac,
            u64::from(self.ac_clear),
            self.smap_error,
            self.smep_error,
            u64::from(self.cr0_wp),
        ];
        for (field, value) in values.into_iter().enumerate() {
            RESULTS[cpu * FIELDS + field].store(value, Ordering::Release);
        }
    }

    fn load(cpu: usize) -> Self {
        let field = |index: usize| RESULTS[cpu * FIELDS + index].load(Ordering::Acquire);
        Self {
            cr4_ok: field(F_CR4) != 0,
            smap_ok: field(F_SMAP) != 0,
            smep_ok: field(F_SMEP) != 0,
            copy_ok: field(F_COPY) != 0,
            read_only_refused: field(F_READ_ONLY) != 0,
            unmapped_refused: field(F_UNMAPPED) != 0,
            partial_ok: field(F_PARTIAL) != 0,
            faults: field(F_FAULTS),
            rejects: field(F_REJECTS),
            handler_ac: field(F_HANDLER_AC),
            ac_clear: field(F_AC_CLEAR) != 0,
            smap_error: field(F_SMAP_ERROR),
            smep_error: field(F_SMEP_ERROR),
            cr0_wp: field(F_CR0_WP) != 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct UserAccessQualification {
    pub cpus: usize,
    pub probes: [CpuAccess; MAX_PROBED_CPUS],
    pub features: Features,
    /// Page de code en lecture seule, page de données inscriptible, toutes
    /// deux U/S à chaque niveau ; page de faute absente.
    pub pages_ok: bool,
}

/// Frames des pages de la sonde, mappées aussi en identité (supervision).
static CODE_FRAME: AtomicU64 = AtomicU64::new(0);
static DATA_FRAME: AtomicU64 = AtomicU64::new(0);

fn address(function: unsafe extern "C" fn()) -> u64 {
    function as *const () as usize as u64
}

/// Appelé par le dispatcher d'exceptions pour une #PF : reprend la faute
/// attendue par la sonde armée sur ce CPU, une seule fois. Rend le RIP et le
/// RSP de reprise.
pub fn on_fault(site: &FaultSite, rsp: u64) -> Option<(u64, u64)> {
    if ring3::from_user(site.cs) || site.vector != user_access::VECTOR_PAGE_FAULT {
        return None;
    }
    let armed = ARMED.load(Ordering::Acquire);
    if armed == ARMED_NONE || PROBE_CPU.load(Ordering::Acquire) != cpu_local::index() as u64 {
        return None;
    }
    let resume = match armed {
        ARMED_SMAP if site.rip == address(elyon_smap_probe_access) && site.cr2 == PROBE_DATA_VA => {
            (address(elyon_smap_probe_resume), rsp)
        }
        ARMED_SMEP if site.rip == PROBE_CODE_VA && rsp & 7 == 0 => {
            // SAFETY: CS noyau : `rsp` est la pile noyau du code interrompu, où
            // le `call` d'elyon_smep_probe vient de pousser son adresse de retour.
            let ret = unsafe { read_volatile(rsp as *const u64) };
            if ret != address(elyon_smep_probe_return) {
                return None;
            }
            (ret, rsp + 8)
        }
        _ => return None,
    };
    if ARMED
        .compare_exchange(armed, ARMED_NONE, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    for (slot, value) in SITE.iter().zip([
        site.vector,
        site.cs,
        site.rip,
        site.error_code,
        site.cr2,
        site.rflags,
    ]) {
        slot.store(value, Ordering::Release);
    }
    FIRED.store(true, Ordering::Release);
    Some(resume)
}

fn arm(kind: u64, cpu: usize) {
    FIRED.store(false, Ordering::Release);
    PROBE_CPU.store(cpu as u64, Ordering::Release);
    ARMED.store(kind, Ordering::Release);
}

/// Faute relevée depuis le dernier `arm`, et désarme.
fn fired_site() -> Option<FaultSite> {
    ARMED.store(ARMED_NONE, Ordering::Release);
    if !FIRED.load(Ordering::Acquire) {
        return None;
    }
    let field = |index: usize| SITE[index].load(Ordering::Acquire);
    Some(FaultSite {
        vector: field(0),
        cs: field(1),
        rip: field(2),
        error_code: field(3),
        cr2: field(4),
        rflags: field(5),
    })
}

fn pattern(cpu: usize) -> [u8; PATTERN_LEN] {
    core::array::from_fn(|index| 0xA5 ^ ((cpu as u8) << 4) ^ index as u8)
}

/// Lit `len` octets d'une frame par le mappage identité du noyau.
fn read_frame(frame: u64, offset: u64, out: &mut [u8]) {
    // SAFETY: frame permanente de la sonde, mappée en identité (supervision) ;
    // `offset + out.len()` reste dans la page.
    unsafe {
        copy_nonoverlapping((frame + offset) as *const u8, out.as_mut_ptr(), out.len());
    }
}

fn probe_this_cpu(cpu: usize) -> CpuAccess {
    let features = cpu::supervisor_protection();
    let code_frame = CODE_FRAME.load(Ordering::Acquire);
    let data_frame = DATA_FRAME.load(Ordering::Acquire);
    let mut result = CpuAccess {
        cr4_ok: user_access::check_cr4(cpu::read_cr4(), features).is_ok(),
        cr0_wp: user_access::check_write_protect(cpu::read_cr0()).is_ok(),
        ..CpuAccess::default()
    };

    // SMAP : lecture directe d'une page utilisateur présente.
    arm(ARMED_SMAP, cpu);
    // SAFETY: la page de données est présente ; sous SMAP, la lecture lève
    // une #PF reprise par `on_fault`, qui rend la marque.
    let value = unsafe { elyon_smap_probe(PROBE_DATA_VA as *const u64) };
    if let Some(site) = fired_site() {
        result.smap_error = site.error_code;
        result.smap_ok =
            value == SMAP_MARK && user_access::check_smap_fault(&site, PROBE_DATA_VA).is_ok();
    }

    // SMEP : appel direct de la page de code utilisateur.
    arm(ARMED_SMEP, cpu);
    // SAFETY: sous SMEP, l'exécution lève une #PF reprise par `on_fault` au
    // retour d'elyon_smep_probe ; sans SMEP, la page rend 2 sans autre effet.
    let returned = unsafe { elyon_smep_probe(PROBE_CODE_VA) };
    if let Some(site) = fired_site() {
        result.smep_error = site.error_code;
        result.smep_ok =
            returned == 0 && user_access::check_smep_fault(&site, PROBE_CODE_VA).is_ok();
    }

    let faults_before = user_copy::faults(cpu);
    let handler_ac_before = user_copy::handler_ac(cpu);

    // Aller-retour d'un motif propre au CPU, après le premier mot.
    let offset = (cpu as u64 + 1) * PATTERN_LEN as u64;
    let sent = pattern(cpu);
    let mut seen = [0u8; PATTERN_LEN];
    let mut back = [0u8; PATTERN_LEN];
    let written = user_copy::copy_to_user(PROBE_DATA_VA + offset, &sent).is_ok();
    read_frame(data_frame, offset, &mut seen);
    let read = user_copy::copy_from_user(&mut back, PROBE_DATA_VA + offset).is_ok();
    result.copy_ok = written && read && seen == sent && back == sent;

    // Écriture sur la page de code en lecture seule : refusée, page intacte.
    let mut code = [0u8; USER_CODE.len()];
    let refused = user_copy::copy_to_user(PROBE_CODE_VA, &[0xCC; 8]) == Err(UserAccessError::Fault);
    read_frame(code_frame, 0, &mut code);
    result.read_only_refused = refused && code == USER_CODE;

    // Page jamais mappée.
    let mut buffer = [0u8; 8];
    result.unmapped_refused =
        user_copy::copy_from_user(&mut buffer, PROBE_UNMAPPED_VA) == Err(UserAccessError::Fault);

    // Lecture qui déborde sur la page jamais mappée : les octets d'avant la
    // faute sont copiés, la copie échoue.
    let mut partial = [0u8; 16];
    let crossing = user_copy::copy_from_user(&mut partial, PROBE_UNMAPPED_VA - 8)
        == Err(UserAccessError::Fault);
    result.partial_ok = crossing && partial[..8] == TAIL && partial[8..] == [0; 8];

    result.faults = user_copy::faults(cpu) - faults_before;
    result.handler_ac = user_copy::handler_ac(cpu) - handler_ac_before;

    // Plages invalides : refusées avant tout accès.
    let faults_before = user_copy::faults(cpu);
    let mut buffer = [0u8; 16];
    let rejects = [
        (
            user_copy::copy_from_user(&mut buffer, data_frame),
            UserAccessError::NotUser,
        ),
        (
            user_copy::copy_from_user(&mut [], PROBE_DATA_VA),
            UserAccessError::Empty,
        ),
        (
            user_copy::copy_from_user(&mut buffer, ring3::USER_BASE - 8),
            UserAccessError::NotUser,
        ),
        (
            user_copy::copy_to_user(ring3::USER_LIMIT - 8, &buffer),
            UserAccessError::NotUser,
        ),
        (
            user_copy::copy_from_user(&mut buffer, u64::MAX - 4),
            UserAccessError::Overflow,
        ),
    ];
    if user_copy::faults(cpu) == faults_before {
        result.rejects = rejects
            .iter()
            .filter(|(outcome, expected)| *outcome == Err(*expected))
            .count() as u64;
    }

    result.ac_clear = cpu::read_rflags() & user_access::RFLAGS_AC == 0;
    result
}

#[unsafe(no_mangle)]
extern "C" fn elyon_user_access_probe_task_entry(arg: u64) -> ! {
    if arg & !0xFFFF != PROBE_ARG_BASE {
        sched::fail_from(SchedError::ArgMismatch);
    }
    cpu::disable_interrupts();
    // SAFETY: recharger CR3 avec sa propre valeur vide le TLB de ce CPU : les
    // pages de la sonde viennent d'être mappées par le BSP.
    unsafe { asm!("mov {0}, cr3", "mov cr3, {0}", out(reg) _, options(nostack, preserves_flags)) };
    let cpu = sched::this_cpu();
    probe_this_cpu(cpu).store(cpu);
    DONE.store(true, Ordering::Release);
    sched::exit_now()
}

/// Alloue et mappe les pages de la sonde, puis relit leurs entrées.
fn map_pages() -> Result<bool, ProbeError> {
    let code = memory::allocate_frame().ok_or(ProbeError::AllocationFailed)?;
    let data = memory::allocate_frame().ok_or(ProbeError::AllocationFailed)?;
    // SAFETY: frames neuves (effacées), mappées en identité, encore
    // inaccessibles au ring 3.
    unsafe {
        copy_nonoverlapping(USER_CODE.as_ptr(), code as *mut u8, USER_CODE.len());
        write_volatile(data as *mut u64, DATA_WORD);
        copy_nonoverlapping(
            TAIL.as_ptr(),
            (data + PAGE_SIZE - TAIL.len() as u64) as *mut u8,
            TAIL.len(),
        );
    }
    CODE_FRAME.store(code, Ordering::Release);
    DATA_FRAME.store(data, Ordering::Release);
    paging::map_user_page(PROBE_CODE_VA, code, false)?;
    paging::map_user_page(PROBE_DATA_VA, data, true)?;
    Ok(paging::check_user_page(PROBE_UNMAPPED_VA, false) == Err(Ring3Error::NotPresent))
}

pub fn qualify(
    bsp_stack_start: u64,
    bsp_stack_end: u64,
    identity_limit: u64,
) -> Result<Option<UserAccessQualification>, ProbeError> {
    cpu::disable_interrupts();
    let cpus = sched::online_cpus();
    // Les pages vivent dans la table de la sonde K005-02, qui ne tourne
    // qu'avec au moins un AP.
    if cpus < 2 {
        return Ok(None);
    }
    if cpus > MAX_PROBED_CPUS {
        return Err(ProbeError::Sched(SchedError::TraceOverflow));
    }
    let pages_ok = map_pages()?;

    let mut probes = [CpuAccess::default(); MAX_PROBED_CPUS];
    probes[0] = probe_this_cpu(0);

    let bsp = StackRegion {
        start: bsp_stack_start,
        end: bsp_stack_end,
    };
    let entry = elyon_user_access_probe_task_entry as *const () as usize as u64;
    for (ap, probe) in probes.iter_mut().enumerate().take(cpus).skip(1) {
        DONE.store(false, Ordering::Release);
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
        *probe = CpuAccess::load(ap);
    }
    PROBE_CPU.store(u64::MAX, Ordering::Release);

    Ok(Some(UserAccessQualification {
        cpus,
        probes,
        features: cpu::supervisor_protection(),
        pages_ok,
    }))
}
