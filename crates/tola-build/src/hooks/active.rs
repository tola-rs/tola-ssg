//! The hook currently calling a synchronous library entry on this thread.

use std::cell::Cell;

use crate::config::section::build::hooks::HookStage;

thread_local! {
    static STAGE: Cell<Option<HookStage>> = const { Cell::new(None) };
}

pub(crate) struct ActiveHook {
    previous: Option<HookStage>,
    // Dropping the guard must restore the thread where it was created.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub(crate) fn enter(stage: HookStage) -> ActiveHook {
    ActiveHook {
        previous: STAGE.replace(Some(stage)),
        _thread: std::marker::PhantomData,
    }
}

pub(crate) fn stage() -> Option<HookStage> {
    STAGE.get()
}

impl Drop for ActiveHook {
    fn drop(&mut self) {
        STAGE.set(self.previous);
    }
}
