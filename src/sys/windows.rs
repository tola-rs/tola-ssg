//! Platform clipboard and terminal output.

use std::io::{self, Write};

/// This system's clipboard command, when the desktop provides one.
///
/// Windows relies on the terminal's own OSC 52 escape alone.
pub(crate) fn clipboard_command() -> Option<&'static str> {
    None
}

/// Write bytes to the process's own standard error, keeping native console/ANSI adaptation.
pub(crate) fn write_process_stderr(bytes: &[u8]) -> io::Result<()> {
    let mut stderr = anstream::AutoStream::always(io::stderr().lock());
    stderr.write_all(bytes)?;
    stderr.flush()
}
