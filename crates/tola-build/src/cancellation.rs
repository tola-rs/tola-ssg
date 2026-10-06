//! Cancellation shared by all stages of one build attempt.

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
        self.cancellation.bundle.cancel();
    }
}

#[derive(Clone)]
pub struct BuildCancellation {
    bundle: BundleCancellation,
}

#[derive(Debug, thiserror::Error)]
#[error("build cancelled")]
pub struct BuildCancelled;

impl BuildCancellation {
    pub fn new() -> Self {
        Self {
            bundle: BundleCancellation::new(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.bundle.is_cancelled()
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
        self.bundle.clone()
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
pub(crate) fn is_cancelled(error: &anyhow::Error) -> bool {
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
    use super::{BuildCancelled, BuildCanceller};

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
}
