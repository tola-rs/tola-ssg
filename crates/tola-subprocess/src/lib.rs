//! Trusted external command execution with bounded output capture, process-tree
//! termination, and cancellation observation.
//!
//! One [`Command`] runs one declared program: it owns the child's process
//! containment (a process group on Unix, a job object on Windows) and terminates
//! that tree when the command ends or the caller's [`Cancellation`] is observed.
//! [`Command::run`] captures and relays bounded output. Explicit
//! [`Command::run_inherited`] instead lends the caller's standard streams to the
//! command and returns its [`Stop`] without capturing output.
//!
//! This crate is a mechanism, not a policy layer: it knows nothing about build
//! stages, declared outputs, configuration files, or how a caller renders what a
//! command printed. It executes *trusted* commands — there is no sandbox — so a
//! command's side effects are its own, and only the guarantees below hold.
//!
//! # Guarantees
//!
//! - Containment is established at spawn, before the command can create children.
//! - [`Command::run`] uses null stdin; stream inheritance is explicit.
//! - [`Command::run_inherited`] gives a Unix child the controlling terminal before
//!   it starts, when a foreground caller inherits a terminal, and restores the
//!   foreground group and terminal settings on return or unwinding.
//! - Captured output is bounded: each stream retains a fixed head and tail, and
//!   reports how many bytes were omitted. Output volume never fails a run.
//! - A failed reader ends observation and releases the process tree without
//!   waiting for the command to exit on its own.
//! - Every started reader thread is joined before [`Command::run`] returns or
//!   unwinds. After the child stops, a drain grace limits further reads even when a
//!   descendant holds a pipe open; already-read output is retained.
//! - Opted-in [`Observer`] callbacks preserve each stream's byte order and complete
//!   each stream once on a successfully observed run. See [`Observer::finished`]
//!   for cancellation and error boundaries.
//! - The child's exit is observed, never assumed: the returned [`Exit`] carries
//!   the status that was waited for, or [`Stop::Cancelled`] when the caller's
//!   cancellation was observed and the tree was terminated.
//! - Cancellation is an outcome, not an error. A cancelled run still reports the
//!   output it captured and any exit status obtained while terminating the child.
//!
//! # Caller obligations
//!
//! - A [`Cancellation`] request is observed by polling, not enforced as a deadline.
//!   OS scheduling, process termination, and observer callbacks can delay return.
//! - Both run methods block the calling thread. The [`Observer`] callbacks run on
//!   that same thread; callers must let them return for polling and draining to
//!   continue.
//! - Terminated commands leave their external effects behind: files they wrote,
//!   network requests they made, and descendants outside the containment domain
//!   (a process that leaves the group with `setpgid` or `setsid`, or a Windows
//!   process created with breakaway rights) are not rolled back or reached.

#![deny(unsafe_code)]

mod cancellation;
mod capture;
mod child;
mod command;
mod error;
mod sys;

pub use cancellation::{Cancellation, NO_CANCELLATION, NoCancellation};
pub use capture::{Captured, Stream};
pub use child::{Exit, Stop};
pub use command::{Command, Observer};
pub use error::{Error, Result};

pub use sys::terminating_signal;
