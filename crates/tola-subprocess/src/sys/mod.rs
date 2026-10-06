//! Platform support: process containment and the pipe readers that observe a stop.
//!
//! Both halves exist because a blocking pipe read cannot observe a stop on its
//! own, and terminating a child must reach the descendants it leaves behind.
//!
//! The readers wrap pipe read ends this crate owns exclusively, so making one
//! nonblocking cannot affect any other user of the same descriptor.

use process_wrap::std::ChildWrapper;
use std::process::ExitStatus;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{Pipe, Reader, wrap};
#[cfg(windows)]
pub(crate) use windows::{Pipe, Reader, wrap};

#[cfg(unix)]
pub use unix::terminating_signal;
#[cfg(windows)]
pub use windows::terminating_signal;

pub(crate) fn terminate(child: &mut dyn ChildWrapper) -> Option<ExitStatus> {
    let _ = child.start_kill();
    // Native polling keeps the direct-child status cache authoritative, so the
    // inner child is killed and reaped here rather than through the wrapper.
    let direct_child = child.inner_mut();
    let _ = direct_child.start_kill();
    direct_child.wait().ok()
}

/// The caller's request to stop reading a stream, shared with its reader.
///
/// Reader shutdown is independent of the run's cancellation so a terminated
/// command retains the same drain opportunity as one that exited by itself.
#[derive(Clone, Default)]
pub(crate) struct ReaderStop(Arc<AtomicBool>);

impl ReaderStop {
    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Raised inside a reader when the stop request interrupted a pending read.
#[derive(Debug, thiserror::Error)]
#[error("the stream reader was stopped")]
pub(crate) struct Stopped;
