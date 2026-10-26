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

use core::ptr::copy_nonoverlapping;

use elyon_abi::PAGE_SIZE;
use elyon_core::user_pages::{PageKind, UserPageError};

use crate::arch::x86_64::paging;
use crate::memory::{self, FrameRange};
use crate::tlb::{self, TlbError};
use crate::{fatal, kprintln};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserMemoryError {
    Page(UserPageError),
    AllocationFailed,
    /// Contenu initial plus grand qu'une page.
    TooLarge,
}

impl UserMemoryError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page(error) => error.as_str(),
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

fn shootdown_or_halt(address: u64) {
    if let Err(error) = tlb::shootdown(address) {
        fail(error);
    }
}

fn fail(error: TlbError) -> ! {
    kprintln!("[user-memory] FAILED reason={}", error.as_str());
    fatal::halt_system()
}

impl UserPage {
    pub fn new(va: u64, kind: PageKind, init: &[u8]) -> Result<Self, UserMemoryError> {
        let range = memory::allocate_owned(1).ok_or(UserMemoryError::AllocationFailed)?;
        Self::from_range(range, va, kind, init)
    }

    /// Comme `new`, avec une frame déjà prise par `allocate_owned` : la
    /// qualification connaît ainsi la frame avant d'en retirer l'alias. En
    /// cas d'erreur, la frame retourne au PMM.
    pub fn from_range(
        range: FrameRange,
        va: u64,
        kind: PageKind,
        init: &[u8],
    ) -> Result<Self, UserMemoryError> {
        // L'entrée utilisateur doit être libre.
        let free_entry = paging::user_entry(va).map(|entry| entry == 0);
        if init.len() as u64 > PAGE_SIZE || free_entry != Ok(true) {
            let _ = memory::free(range);
            return Err(match free_entry {
                Err(error) => error.into(),
                Ok(false) => UserMemoryError::Page(UserPageError::Leaf),
                Ok(true) => UserMemoryError::TooLarge,
            });
        }
        let frame = range.start();
        // SAFETY: frame neuve et effacée, encore atteinte par son seul alias
        // identité, de taille PAGE_SIZE >= `init.len()`.
        unsafe { copy_nonoverlapping(init.as_ptr(), frame as *mut u8, init.len()) };
        if let Err(error) = paging::remove_identity_alias(frame) {
            // L'alias est intact : la frame peut retourner au PMM.
            let _ = memory::free(range);
            return Err(error.into());
        }
        shootdown_or_halt(frame);
        if let Err(error) = paging::map_user_leaf(va, frame, kind) {
            if paging::restore_identity_alias(frame).is_ok() {
                let _ = memory::free(range);
            }
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

    /// Page permanente : jamais libérée, sa frame reste au ring 3. Le jeton
    /// de la frame est abandonné sans `free` : elle reste marquée allouée.
    pub fn keep(self) {
        let Self { range, .. } = self;
        let _permanent = range;
    }

    /// Retire la page du ring 3 sur tous les CPU, puis rend la frame.
    pub fn release(self) -> Result<(), UserMemoryError> {
        let frame = self.frame();
        if paging::unmap_user_leaf(self.va)? != frame {
            return Err(UserMemoryError::Page(UserPageError::Leaf));
        }
        // Aucune traduction utilisateur ne doit survivre au retour de la
        // frame au PMM.
        shootdown_or_halt(self.va);
        paging::restore_identity_alias(frame)?;
        memory::free(self.range).map_err(|_| UserMemoryError::AllocationFailed)
    }
}
