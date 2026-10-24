//! Pages rendables de chaque tâche, pile et zone FPU, gardées par slot jusqu'à
//! sa récupération (K004-11).
//!
//! Les jetons `FrameRange` rangés ici sont les seuls à pouvoir rendre ces
//! pages au PMM. Le verrou est de classe `Pmm` et n'est jamais tenu pendant un
//! appel au PMM : un jeton est sorti du registre, puis rendu verrou relâché.

use elyon_core::lock_order::LockClass;

use crate::memory::{self, FrameRange, PmmError};
use crate::sync::IrqSpinLock;
use crate::task::{self, MAX_TASKS, TaskError, TaskId};

struct Frames {
    stack: Option<FrameRange>,
    fpu: Option<FrameRange>,
}

impl Frames {
    const EMPTY: Self = Self {
        stack: None,
        fpu: None,
    };
}

static FRAMES: IrqSpinLock<[Frames; MAX_TASKS]> =
    IrqSpinLock::new(LockClass::Pmm, [const { Frames::EMPTY }; MAX_TASKS]);

#[derive(Clone, Copy)]
pub enum FramesError {
    Task(TaskError),
    Pmm(PmmError),
    /// Le slot détient déjà une plage de ce type : la nouvelle a été rendue.
    AlreadyOwned,
}

impl FramesError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task(error) => error.as_str(),
            Self::Pmm(PmmError::NotFreeable) => "task frames are not freeable",
            Self::AlreadyOwned => "task slot already owns frames of this kind",
        }
    }
}

impl From<TaskError> for FramesError {
    fn from(error: TaskError) -> Self {
        Self::Task(error)
    }
}

impl From<PmmError> for FramesError {
    fn from(error: PmmError) -> Self {
        Self::Pmm(error)
    }
}

pub fn keep_stack(id: TaskId, range: FrameRange) -> Result<(), FramesError> {
    keep(id, range, |frames| &mut frames.stack)
}

pub fn keep_fpu(id: TaskId, range: FrameRange) -> Result<(), FramesError> {
    keep(id, range, |frames| &mut frames.fpu)
}

fn keep(
    id: TaskId,
    range: FrameRange,
    cell: impl FnOnce(&mut Frames) -> &mut Option<FrameRange>,
) -> Result<(), FramesError> {
    let index = task::slot_index(id)?;
    let refused = {
        let mut table = FRAMES.lock();
        let cell = cell(&mut table[index]);
        if cell.is_some() {
            Some(range)
        } else {
            *cell = Some(range);
            None
        }
    };
    match refused {
        Some(range) => {
            memory::free(range)?;
            Err(FramesError::AlreadyOwned)
        }
        None => Ok(()),
    }
}

/// Rend au PMM la pile et la zone FPU d'une tâche en cours de récupération.
/// L'appelant a retiré la pile du slot (`task::release_stack`) : aucune autre
/// tâche ne s'exécute dessus ni ne la détient.
pub fn release(id: TaskId) -> Result<(), FramesError> {
    let index = task::slot_index(id)?;
    let frames = {
        let mut table = FRAMES.lock();
        core::mem::replace(&mut table[index], Frames::EMPTY)
    };
    // Les deux plages sont rendues avant de signaler un échec : la première
    // erreur est rapportée, aucune plage n'est abandonnée en silence.
    let mut result = Ok(());
    for range in [frames.stack, frames.fpu].into_iter().flatten() {
        if let Err(error) = memory::free(range) {
            result = result.and(Err(FramesError::Pmm(error)));
        }
    }
    result
}
