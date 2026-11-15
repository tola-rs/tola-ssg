//! Cancellation shared by all stages of one build attempt.

use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use tola_typst::BundleCancellation;

#[derive(Clone, Default)]
pub struct BuildCanceller {
    cancellation: BuildCancellation,
}

impl BuildCanceller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn token(&self) -> BuildCancellation {
        self.cancellation.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.signal.cancel(StopSource::Requested);
    }

    /// Create a separately cancellable scope that also stops with `parent`.
    pub fn child(parent: &BuildCancellation) -> Self {
        Self {
            cancellation: BuildCancellation::linked(&[parent]),
        }
    }
}

#[derive(Clone)]
pub struct BuildCancellation {
    signal: Arc<Signal>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopSource {
    Requested,
    Parent,
}

struct Signal {
    flag: Arc<AtomicBool>,
    state: Mutex<SignalState>,
    // Keep propagation intact after intermediate handles have been dropped.
    _parents: Box<[Arc<Signal>]>,
}

#[derive(Default)]
struct SignalState {
    source: Option<StopSource>,
    children: Vec<Weak<Signal>>,
    adapters: Vec<Weak<AtomicBool>>,
}

impl Signal {
    fn cancel(&self, source: StopSource) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.source.get_or_insert(source);
        self.flag.store(true, Ordering::Relaxed);
        state.adapters.retain(|flag| {
            let Some(flag) = flag.upgrade() else {
                return false;
            };
            flag.store(true, Ordering::Relaxed);
            true
        });
        // Each link points to a newer scope, so locks follow the same order.
        state.children.retain(|child| {
            let Some(child) = child.upgrade() else {
                return false;
            };
            child.cancel(StopSource::Parent);
            true
        });
    }

    fn add_child(&self, child: &Arc<Signal>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if self.flag.load(Ordering::Relaxed) {
            child.cancel(StopSource::Parent);
        }
        state.children.retain(|child| child.strong_count() > 0);
        state.children.push(Arc::downgrade(child));
    }
}

#[derive(Debug, thiserror::Error)]
#[error("build cancelled")]
pub struct BuildCancelled;

impl BuildCancellation {
    pub fn new() -> Self {
        Self::linked(&[])
    }

    pub(crate) fn either(&self, other: &Self) -> Self {
        if Arc::ptr_eq(&self.signal, &other.signal) {
            self.clone()
        } else {
            Self::linked(&[self, other])
        }
    }

    fn linked(parents: &[&Self]) -> Self {
        let signal = Arc::new(Signal {
            flag: Arc::new(AtomicBool::new(false)),
            state: Mutex::default(),
            _parents: parents
                .iter()
                .map(|parent| Arc::clone(&parent.signal))
                .collect(),
        });
        for parent in parents {
            parent.signal.add_child(&signal);
        }
        Self { signal }
    }

    pub(crate) fn was_cancelled_directly(&self) -> bool {
        self.signal
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .source
            == Some(StopSource::Requested)
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag().load(Ordering::Relaxed)
    }

    /// The flag `tola_input` readers poll, so one read stops with this attempt's cancellation.
    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.signal.flag
    }

    /// Give an adapter an interrupt flag it can reset without resetting this scope.
    pub fn signal_flag(&self) -> Arc<AtomicBool> {
        let mut state = self
            .signal
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let flag = Arc::new(AtomicBool::new(self.is_cancelled()));
        state.adapters.retain(|flag| flag.strong_count() > 0);
        state.adapters.push(Arc::downgrade(&flag));
        flag
    }

    pub fn ensure_active(&self) -> Result<(), BuildCancelled> {
        if self.is_cancelled() {
            Err(BuildCancelled)
        } else {
            Ok(())
        }
    }

    /// The Typst-side handle sharing this attempt's cancellation state, for APIs that take
    /// their own cancellation type.
    pub(crate) fn bundle_cancellation(&self) -> BundleCancellation {
        BundleCancellation::from_flag(Arc::clone(&self.signal.flag))
    }
}

impl tola_image::Cancellation for BuildCancellation {
    fn is_cancelled(&self) -> bool {
        Self::is_cancelled(self)
    }
}

/// Whether an error chain has a cancellation this engine observes.
///
/// Every stage returns cancellation as a typed cause, so classifying one is a property of the
/// error rather than of the stage that produced it.
pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<BuildCancelled>()
            || cause.is::<tola_input::Cancelled>()
            || cause.is::<tola_image::ImageCancelled>()
            || cause
                .downcast_ref::<tola_typst::FontLoadError>()
                .is_some_and(|error| error.is_cancelled())
            || matches!(
                cause.downcast_ref::<tola_typst::CompileError>(),
                Some(tola_typst::CompileError::Cancelled)
            )
    })
}

/// Cancellation checks for stages that may not receive a build cancellation.
pub(crate) trait OptionalCancellation {
    /// Reject the stage when a cancellation was supplied and has already been requested.
    fn ensure_active_if_present(&self) -> Result<(), BuildCancelled>;
}

impl OptionalCancellation for Option<&BuildCancellation> {
    fn ensure_active_if_present(&self) -> Result<(), BuildCancelled> {
        match self {
            Some(cancellation) => cancellation.ensure_active(),
            None => Ok(()),
        }
    }
}

impl Default for BuildCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildCancelled, BuildCanceller, is_cancelled};
    use std::sync::atomic::Ordering;

    #[test]
    fn attempt_cancellation_stays_scoped() {
        let superseded = BuildCanceller::new();
        let current = BuildCanceller::new();
        let superseded_worker = superseded.token();
        let current_worker = current.token();
        let child = BuildCanceller::child(&superseded_worker);
        let grandchild = BuildCanceller::child(&child.token()).token();
        let adapter = grandchild.signal_flag();
        let joined = grandchild.either(&current_worker);
        drop(child);

        superseded.cancel();

        assert!(matches!(
            superseded_worker.ensure_active(),
            Err(BuildCancelled)
        ));
        assert!(current_worker.ensure_active().is_ok());
        assert!(grandchild.is_cancelled());
        assert!(joined.is_cancelled());
        assert!(!grandchild.was_cancelled_directly());
        assert!(adapter.swap(false, Ordering::Relaxed));
        assert!(grandchild.is_cancelled());
        superseded.cancel();
        assert!(adapter.load(Ordering::Relaxed));
        let late = BuildCanceller::child(&superseded_worker);
        late.cancel();
        assert!(!late.token().was_cancelled_directly());
    }

    #[test]
    fn cancelled_failure_keeps_attached_diagnostics() {
        let diagnostics = vec![
            tola_diagnostic::Diagnostic::new(
                tola_diagnostic::codes::typst::SOURCE_ANALYSIS,
                tola_diagnostic::Severity::Error,
                "the check stopped before the site was analyzed",
            )
            .with_note("the previous revision stays published"),
        ];

        let error = anyhow::Error::new(tola_diagnostic::DiagnosticError::attach(
            anyhow::Error::new(BuildCancelled),
            diagnostics,
        ));

        assert!(is_cancelled(&error));
        assert_eq!(error.to_string(), "build cancelled");
        let attached = tola_diagnostic::attached(&error).expect("the diagnostics stay attached");
        assert_eq!(attached.len(), 1);
        assert_eq!(
            attached[0].code,
            tola_diagnostic::codes::typst::SOURCE_ANALYSIS
        );
        assert_eq!(attached[0].notes.len(), 1);
    }
}
