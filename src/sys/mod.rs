//! The operating system's own behavior for the command's files, directories, and streams,
//! behind one interface.
//!
//! Each operation below has one implementation per platform family, and a caller names the
//! operation rather than the platform: no `cfg` reaches the code that uses it.

use std::io;
use std::process::{Child, Command, ExitStatus};

use tola_build::cancellation::BuildCancellation;

/// Wait for a command attached to the caller's terminal without blocking cancellation.
pub(crate) fn run_command(
    command: &mut Command,
    cancellation: &BuildCancellation,
) -> io::Result<ExitStatus> {
    run_with_input(command, None, cancellation)
}

/// Write `input` to the command's standard input and wait for it, without blocking
/// cancellation.
pub(crate) fn run_command_with_input(
    command: &mut Command,
    input: &[u8],
    cancellation: &BuildCancellation,
) -> io::Result<ExitStatus> {
    run_with_input(command, Some(input), cancellation)
}

fn run_with_input(
    command: &mut Command,
    input: Option<&[u8]>,
    cancellation: &BuildCancellation,
) -> io::Result<ExitStatus> {
    use std::io::Write as _;

    cancellation.ensure_active().map_err(io::Error::other)?;
    if input.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = CommandChild(command.spawn()?);
    if let Some(input) = input
        && let Some(mut stdin) = child.0.stdin.take()
    {
        // Dropping the pipe ends the input: a command that reads it to the end can finish.
        let _ = stdin.write_all(input);
    }
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        if let Err(cancelled) = cancellation.ensure_active() {
            child.stop()?;
            return Err(io::Error::other(cancelled));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

struct CommandChild(Child);

impl CommandChild {
    fn stop(&mut self) -> io::Result<()> {
        if self.0.try_wait()?.is_some() {
            return Ok(());
        }
        if let Err(error) = self.0.kill() {
            return match self.0.try_wait()? {
                Some(_) => Ok(()),
                None => Err(error),
            };
        }
        self.0.wait().map(|_| ())
    }
}

impl Drop for CommandChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{
    clipboard_command, create_new_file, link_count, link_directory, link_removes_as_directory,
    open_default, open_directory_nofollow, open_file_for_read, open_filesystem_root,
    open_log_for_append, open_log_for_read, rename_without_replacing, write_process_stderr,
};
#[cfg(windows)]
pub(crate) use windows::{
    clipboard_command, create_new_file, link_count, link_directory, link_removes_as_directory,
    open_default, open_directory_nofollow, open_file_for_read, open_filesystem_root,
    open_log_for_append, open_log_for_read, rename_without_replacing, write_process_stderr,
};
