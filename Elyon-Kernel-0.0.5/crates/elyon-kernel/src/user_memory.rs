//! K005-05 : pages utilisateur.
//!
//! Une `UserPage` possède sa frame, prise par `memory::allocate_owned`. À la
//! création : contenu initial écrit par le mappage identité, alias identité
//! retiré et invalidé sur tous les CPU, puis mappage utilisateur aux droits
//! exacts de son type. Dès lors, le noyau n'atteint la page que par
//! `user_copy`, sous SMAP ; un accès par l'alias lève une #PF. À la
//! libération : mappage utilisateur retiré et invalidé partout, alias rendu,
//! frame rendue au PMM. Un shootdown en échec laisse une traduction possible
//! sur un autre CPU : le système s'arrête alors fail-closed.

use core::mem::ManuallyDrop;
use core::ptr::copy_nonoverlapping;

use elyon_abi::PAGE_SIZE;
use elyon_core::user_pages::{PageKind, UserPageError};

use crate::arch::x86_64::paging;
use crate::kprintln;
use crate::memory::{self, FrameRange, PmmError};
use crate::tlb;

/// Erreur de création ou de libération d'une page utilisateur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserMemoryError {
    Page(UserPageError),
    Pmm(PmmError),
    AllocationFailed,
    /// Contenu initial plus grand qu'une page.
    TooLarge,
}

impl UserMemoryError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page(error) => error.as_str(),
            Self::Pmm(_) => "PMM refused to take the user frame back",
            Self::AllocationFailed => "PMM could not allocate a user page",
            Self::TooLarge => "initial content exceeds one page",
        }
    }
}

impl From<UserPageError> for UserMemoryError {
    fn from(error: UserPageError) -> Self {
        Self::Page(error)
    }
}

/// Page utilisateur mappée en `va`. Sa frame n'est atteinte que par le
/// mappage utilisateur tant qu'elle existe.
#[must_use = "une page non libérée reste mappée : appeler release ou keep"]
pub struct UserPage {
    va: u64,
    kind: PageKind,
    range: FrameRange,
}

/// Rend au PMM une frame dont l'alias identité est présent et qu'aucun
/// mappage utilisateur ne désigne plus sur aucun CPU.
fn give_back(range: FrameRange) -> Result<(), UserMemoryError> {
    memory::free(range).map_err(UserMemoryError::Pmm)
}

/// Frame dont l'état ne permet plus de la rendre sans risque : elle reste
/// allouée pour toujours, et la fuite est signalée.
fn leak(range: FrameRange, reason: UserMemoryError) -> UserMemoryError {
    kprintln!(
        "[user-memory] leaked frame={:#018x} reason={}",
        range.start(),
        reason.as_str()
    );
    let _ = ManuallyDrop::new(range);
    reason
}

impl UserPage {
    pub fn new(va: u64, kind: PageKind, init: &[u8]) -> Result<Self, UserMemoryError> {
        let range = memory::allocate_owned(1).ok_or(UserMemoryError::AllocationFailed)?;
        Self::from_range(range, va, kind, init)
    }

    /// Comme `new`, avec une frame déjà prise par `allocate_owned` : la
    /// qualification connaît ainsi la frame avant d'en retirer l'alias. En
    /// cas d'erreur, la frame retourne au PMM si aucun mappage utilisateur
    /// ne peut plus l'atteindre, sinon elle reste allouée.
    pub fn from_range(
        range: FrameRange,
        va: u64,
        kind: PageKind,
        init: &[u8],
    ) -> Result<Self, UserMemoryError> {
        if init.len() as u64 > PAGE_SIZE {
            give_back(range)?;
            return Err(UserMemoryError::TooLarge);
        }
        let frame = range.start();
        // SAFETY: frame neuve et effacée, encore atteinte par son seul alias
        // identité, de taille PAGE_SIZE >= `init.len()`.
        unsafe { copy_nonoverlapping(init.as_ptr(), frame as *mut u8, init.len()) };
        if let Err(error) = paging::remove_identity_alias(frame) {
            // Alias déjà absent : la frame n'est pas dans l'état attendu, elle
            // reste allouée. Sinon l'alias est intact et elle peut retourner
            // au PMM.
            if error == UserPageError::NotPresent {
                return Err(leak(range, error.into()));
            }
            give_back(range)?;
            return Err(error.into());
        }
        tlb::shootdown(frame);
        if let Err(error) = paging::map_user_leaf(va, frame, kind) {
            // `map_user_leaf` a retiré toute feuille écrite ; une traduction
            // spéculative ailleurs disparaît avec le shootdown.
            tlb::shootdown(va);
            if let Err(restore) = paging::restore_identity_alias(frame) {
                return Err(leak(range, restore.into()));
            }
            give_back(range)?;
            return Err(error.into());
        }
        Ok(Self { va, kind, range })
    }

    pub fn frame(&self) -> u64 {
        self.range.start()
    }

    /// Droits du mappage utilisateur et absence d'alias identité, relus.
    pub fn check(&self) -> Result<(), UserPageError> {
        paging::check_user_page(self.va, self.frame(), self.kind)?;
        if paging::identity_entry(self.frame())? != 0 {
            return Err(UserPageError::AliasPresent);
        }
        Ok(())
    }

    /// Page permanente : jamais libérée, sa frame reste au ring 3 et allouée.
    pub fn keep(self) {
        let _ = ManuallyDrop::new(self.range);
    }

    /// Retire la page du ring 3 sur tous les CPU, puis rend la frame.
    pub fn release(self) -> Result<(), UserMemoryError> {
        let frame = self.frame();
        if let Err(error) = paging::unmap_user_leaf(self.va, frame) {
            // L'entrée ne désigne pas cette frame : rien n'a été retiré, la
            // frame reste à son propriétaire actuel.
            return Err(leak(self.range, error.into()));
        }
        // Aucune traduction utilisateur ne doit survivre au retour de la
        // frame au PMM.
        tlb::shootdown(self.va);
        if let Err(error) = paging::restore_identity_alias(frame) {
            return Err(leak(self.range, error.into()));
        }
        give_back(self.range)
    }
}
