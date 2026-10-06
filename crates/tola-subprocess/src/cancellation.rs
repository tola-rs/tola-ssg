//! The caller-owned cancellation request one run observes.

/// Whether the caller has requested that a running command stop.
///
/// Clones must observe the same request state. Checks are expected to be cheap
/// and non-blocking: a run polls this value between readiness waits, and it never
/// waits for the work to stop.
pub trait Cancellation {
    fn is_cancelled(&self) -> bool;
}

impl<F: Fn() -> bool> Cancellation for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// A cancellation handle that stays idle: a run under it is never cancelled.
#[derive(Clone, Copy, Debug)]
pub struct NoCancellation;

impl Cancellation for NoCancellation {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// The shared idle handle: pass `&NO_CANCELLATION` when no caller can request a stop.
pub const NO_CANCELLATION: NoCancellation = NoCancellation;
