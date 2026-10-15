//! Cancellation shared by all stages of one build attempt.

use std::sync::{
    Arc,
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
        self.cancellation.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct BuildCancellation {
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, thiserror::Error)]
#[error("build cancelled")]
pub struct BuildCancelled;

impl BuildCancellation {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.cancelled
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
        BundleCancellation::from_flag(self.cancelled.clone())
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

    #[test]
    fn attempt_cancellation_stays_scoped() {
        let superseded = BuildCanceller::new();
        let current = BuildCanceller::new();
        let superseded_worker = superseded.token();
        let current_worker = current.token();

        superseded.cancel();

        assert!(matches!(
            superseded_worker.ensure_active(),
            Err(BuildCancelled)
        ));
        assert!(current_worker.ensure_active().is_ok());
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
