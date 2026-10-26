//! K005-04 : copies validées entre espace utilisateur et noyau.
//!
//! SMAP interdit au noyau tout accès aux pages utilisateur ; la seule
//! fenêtre `stac`/`clac` du noyau est l'instruction de copie de
//! `elyon_user_copy`, atteinte seulement avec une plage validée par
//! `user_access::user_range`. Une #PF sur cette instruction (page absente,
//! écriture sur une page en lecture seule) reprend au point de reprise, qui
//! referme la fenêtre et rend le nombre d'octets non copiés : la copie échoue
//! proprement au lieu d'arrêter le système.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use elyon_core::user_access::{self, FaultSite, UserAccessError};

use crate::arch::x86_64::{cpu, cpu_local};
use crate::sched;

global_asm!(
    r#"
// u64 elyon_user_copy(u8 *dst, const u8 *src, u64 len)
// L'un des deux côtés est une plage utilisateur validée, l'autre un tampon
// noyau. DF est nul (ABI). Rend 0, ou les octets restants après une #PF.
.global elyon_user_copy
elyon_user_copy:
    mov rcx, rdx
    stac
.global elyon_user_copy_access
elyon_user_copy_access:
    rep movsb
    clac
    xor eax, eax
    ret
// Point de reprise d'une #PF sur `rep movsb` : RCX porte les octets restants.
.global elyon_user_copy_fixup
elyon_user_copy_fixup:
    clac
    mov rax, rcx
    ret
"#
);

unsafe extern "C" {
    fn elyon_user_copy(dst: *mut u8, src: *const u8, len: u64) -> u64;
    fn elyon_user_copy_access();
    fn elyon_user_copy_fixup();
}

/// Fautes de copie reprises, par CPU.
static FAULTS: [AtomicU64; sched::MAX_SCHED_CPUS] =
    [const { AtomicU64::new(0) }; sched::MAX_SCHED_CPUS];
/// Fautes reprises dont le gestionnaire tournait avec RFLAGS.AC levé : le
/// `clac` d'entrée manque.
static HANDLER_AC: [AtomicU64; sched::MAX_SCHED_CPUS] =
    [const { AtomicU64::new(0) }; sched::MAX_SCHED_CPUS];

fn address(function: unsafe extern "C" fn()) -> u64 {
    function as *const () as usize as u64
}

/// Copie `dst.len()` octets depuis l'adresse utilisateur `src`.
pub fn copy_from_user(dst: &mut [u8], src: u64) -> Result<(), UserAccessError> {
    let range = user_access::user_range(src, dst.len() as u64)?;
    // SAFETY: `range` est entièrement dans la région utilisateur, jamais
    // mappée par le noyau pour lui-même : une page absente ou refusée lève une
    // #PF reprise par `resume`. `dst` est un tampon noyau valide de `len` octets.
    let left = unsafe { elyon_user_copy(dst.as_mut_ptr(), range.start as *const u8, range.len) };
    if left != 0 {
        return Err(UserAccessError::Fault);
    }
    Ok(())
}

/// Copie `src` vers l'adresse utilisateur `dst`.
pub fn copy_to_user(dst: u64, src: &[u8]) -> Result<(), UserAccessError> {
    let range = user_access::user_range(dst, src.len() as u64)?;
    // SAFETY: comme `copy_from_user`, sens inverse ; `src` est un tampon
    // noyau valide de `len` octets.
    let left = unsafe { elyon_user_copy(range.start as *mut u8, src.as_ptr(), range.len) };
    if left != 0 {
        return Err(UserAccessError::Fault);
    }
    Ok(())
}

/// Point de reprise d'une #PF sur l'instruction de copie, ou `None`.
/// Appelé par le dispatcher d'exceptions, GS noyau chargé.
pub fn resume(site: &FaultSite) -> Option<u64> {
    let resume = user_access::copy_resume(
        site,
        address(elyon_user_copy_access),
        address(elyon_user_copy_fixup),
    )?;
    let cpu = cpu_local::index();
    if cpu < sched::MAX_SCHED_CPUS {
        FAULTS[cpu].fetch_add(1, Ordering::AcqRel);
        if cpu::read_rflags() & user_access::RFLAGS_AC != 0 {
            HANDLER_AC[cpu].fetch_add(1, Ordering::AcqRel);
        }
    }
    Some(resume)
}

pub fn faults(cpu: usize) -> u64 {
    FAULTS[cpu].load(Ordering::Acquire)
}

pub fn handler_ac(cpu: usize) -> u64 {
    HANDLER_AC[cpu].load(Ordering::Acquire)
}
