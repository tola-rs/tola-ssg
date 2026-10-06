//! Failures that prevented a command from running or being observed.

use std::io;

/// The reason a command could not be run to an observable end.
///
/// A command that ran and exited unsuccessfully is not an error: [`Exit`](crate::Exit)
/// carries its status, and the caller decides what that status means.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("the command could not be started")]
    Spawn(#[source] io::Error),
    /// The child's output pipes were not available to capture.
    #[error("the command's output could not be captured")]
    Capture(#[source] io::Error),
    #[error("a stream reader could not be prepared")]
    Reader(#[source] io::Error),
    #[error("the command's exit could not be observed")]
    Wait(#[source] io::Error),
    /// A reader failed while draining one of the child's streams.
    #[error("the command's output could not be read")]
    Read(#[source] io::Error),
}

impl Error {
    /// The underlying native failure.
    pub fn source_io(&self) -> &io::Error {
        match self {
            Self::Spawn(error)
            | Self::Capture(error)
            | Self::Reader(error)
            | Self::Wait(error)
            | Self::Read(error) => error,
        }
    }
}

/// The result of one command run.
pub type Result<T, E = Error> = std::result::Result<T, E>;
