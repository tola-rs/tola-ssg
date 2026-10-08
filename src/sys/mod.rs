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
    cancellation.ensure_active().map_err(io::Error::other)?;
    let mut child = CommandChild(command.spawn()?);
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
    create_new_file, link_count, link_directory, link_removes_as_directory, open_default,
    open_directory_nofollow, open_file_for_read, open_filesystem_root, open_log_for_append,
    open_log_for_read, rename_without_replacing, write_process_stderr,
};
#[cfg(windows)]
pub(crate) use windows::{
    create_new_file, link_count, link_directory, link_removes_as_directory, open_default,
    open_directory_nofollow, open_file_for_read, open_filesystem_root, open_log_for_append,
    open_log_for_read, rename_without_replacing, write_process_stderr,
};
