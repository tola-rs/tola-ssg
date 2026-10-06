//! Cancellation shared by one Bundle compilation and export pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cancellation state shared by one Bundle compilation pipeline.
///
/// Typst's realization and export APIs are synchronous, so cancellation is
/// checked at the adapter boundaries around those calls.
#[derive(Clone)]
pub struct BundleCancellation {
    cancelled: Arc<AtomicBool>,
}

impl BundleCancellation {
    /// Create an active cancellation handle.
    ///
    /// Clones share the same flag. Call [`Self::cancel`] from any thread;
    /// it is checked before and after synchronous Typst calls.
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a cancellation handle backed by an atomic flag.
    ///
    /// The caller can set the flag from another thread.
    pub fn from_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Use an explicit cancellation flag; the operation is discarded once the flag is set.
    pub fn with_cancellation(mut self, cancelled: Arc<AtomicBool>) -> Self {
        self.cancelled = cancelled;
        self
    }

    /// Mark this operation as cancelled.
    pub fn cancel(&self) {
        // The flag publishes no other state, so relaxed ordering is sufficient.
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Return a cancellation error if cancellation has been requested.
    pub fn ensure_active(&self) -> Result<(), crate::diagnostic::CompileError> {
        self.ensure_active_as()
    }

    /// Return the caller's cancellation error if cancellation has been requested.
    pub(crate) fn ensure_active_as<E: crate::diagnostic::CancelledError>(&self) -> Result<(), E> {
        if self.is_cancelled() {
            Err(E::from_cancellation())
        } else {
            Ok(())
        }
    }
}

impl Default for BundleCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::CompileError;
    #[test]
    fn cloned_cancellation_shares_the_flag() {
        let cancellation = BundleCancellation::new();
        let worker = cancellation.clone();
        assert!(!cancellation.is_cancelled());
        worker.cancel();
        assert!(cancellation.is_cancelled());
        assert!(matches!(
            cancellation.ensure_active(),
            Err(CompileError::Cancelled)
        ));
    }
}
