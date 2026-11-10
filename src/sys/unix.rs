//! Platform clipboard and terminal output.

use std::io::{self, Write};

/// The command that reads text on its standard input and puts it on this system's
/// clipboard, when the system has one.
///
/// The other systems rely on the terminal's own OSC 52 escape alone.
pub(crate) fn clipboard_command() -> Option<&'static str> {
    cfg!(target_os = "macos").then_some("pbcopy")
}

/// Write bytes to the process's own standard error.
pub(crate) fn write_process_stderr(bytes: &[u8]) -> io::Result<()> {
    let mut stderr = io::stderr();
    stderr.write_all(bytes)?;
    stderr.flush()
}
