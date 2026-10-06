//! The caller-owned cancellation request image processing observes.

/// Whether the caller has requested that image processing stop.
///
/// Clones must observe the same request state. Checks are expected to be cheap
/// and non-blocking: processing polls this value between codec calls, and it
/// never waits for the work to stop. Rows of one resize are checked on the
/// threads that compute them, so an implementation is shared across threads.
pub trait Cancellation: Sync {
    /// Whether the caller has requested the stop. Every call asks again, so a request that
    /// arrives while work is running is still observed.
    fn is_cancelled(&self) -> bool;

    /// Reject the stage once a cancellation has been requested.
    fn ensure_active(&self) -> Result<(), ImageCancelled> {
        if self.is_cancelled() {
            Err(ImageCancelled)
        } else {
            Ok(())
        }
    }
}

impl<F: Fn() -> bool + Sync> Cancellation for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// A cancellation handle that stays idle: work run under it is never cancelled.
#[derive(Clone, Copy, Debug)]
pub struct NoCancellation;

impl Cancellation for NoCancellation {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// The shared idle handle: pass `&NO_CANCELLATION` when no caller can request a stop.
pub const NO_CANCELLATION: NoCancellation = NoCancellation;

/// Reject the stage when a cancellation was supplied and has already been requested.
pub fn ensure_active_if_present(
    cancellation: Option<&dyn Cancellation>,
) -> Result<(), ImageCancelled> {
    match cancellation {
        Some(cancellation) => cancellation.ensure_active(),
        None => Ok(()),
    }
}

/// Image processing stopped because the caller cancelled the attempt that owns it.
///
/// A cancelled render never returns bytes, and callers must recognize this value
/// rather than treat it as a codec failure.
#[derive(Debug, thiserror::Error)]
#[error("image processing cancelled")]
pub struct ImageCancelled;
