//! Trusted external command execution with bounded capture, process-tree
//! termination, and polled cancellation.
//!
//! - Containment starts at spawn (Unix process group, Windows job object) and
//!   terminates the tree on exit, error, unwind, or observed [`Cancellation`].
//! - [`Command::run`] uses null stdin, relays stream chunks to [`Observer`] in
//!   per-stream order on the calling thread, and captures a bounded head and tail.
//! - After child stop, readers drain until EOF or grace/byte limits; every
//!   started reader thread joins before [`Command::run`] returns or unwinds.
//! - [`Command::run_inherited`] inherits caller stdio without capture and, on
//!   Unix, hands off and restores the foreground controlling terminal.
//! - Cancellation is polled, not a hard deadline; external side effects and
//!   descendants that escape containment are not rolled back.

#![deny(unsafe_code)]

mod cancellation;
mod capture;
mod child;
mod command;
mod error;
mod relay;
mod sys;

pub use cancellation::{Cancellation, NO_CANCELLATION, NoCancellation};
pub use capture::{Captured, Stream};
pub use child::{Exit, Stop};
pub use command::{Command, Observer};
pub use error::{Error, Result};

pub use sys::terminating_signal;
