//! Unix: link-free opens, hard-link counts, symlink directories, raw streams, and no-clobber publication.

use std::ffi::OsStr;
use std::fs::{File, FileType, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use cap_std::fs::Dir;

/// The command that reads text on its standard input and puts it on this system's
/// clipboard, when the system has one.
///
/// The other systems rely on the terminal's own OSC 52 escape alone.
pub(crate) fn clipboard_command() -> Option<&'static str> {
    cfg!(target_os = "macos").then_some("pbcopy")
}

/// Pass a path or URL as one argument to the desktop's opener.
pub(crate) fn open_default(
    target: &OsStr,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = super::run_command(
        std::process::Command::new(program)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
        cancellation,
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            "the desktop opener did not accept the target",
        ))
    }
}

/// Open one log path for reading without following links or waiting on a FIFO.
pub(crate) fn open_log_for_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    refuse_links_and_fifos(&mut options);
    options.open(path)
}

/// Configure one log open so it neither blocks on a raced FIFO nor follows a replacement link.
pub(crate) fn open_log_for_append(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
    refuse_links_and_fifos(options);
}

fn refuse_links_and_fifos(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32);
}

/// The number of paths that name one opened file.
pub(crate) fn link_count(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    Ok(file.metadata()?.nlink())
}

/// Create one directory link naming `target`.
pub(crate) fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Whether this link-like entry must be removed as a directory rather than as a file.
pub(crate) fn link_removes_as_directory(_file_type: &FileType) -> bool {
    false
}

/// Write bytes to the process's own standard error.
pub(crate) fn write_process_stderr(bytes: &[u8]) -> io::Result<()> {
    let mut stderr = io::stderr();
    stderr.write_all(bytes)?;
    stderr.flush()
}

/// Open the filesystem root that begins an absolute path.
pub(crate) fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

/// Open one directory beneath an already-open parent, refusing a final link.
pub(crate) fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    parent.open_dir_nofollow(name)
}

/// Open one existing file beneath an already-open directory for reading.
///
/// A final link is refused and a FIFO is never waited on.
pub(crate) fn open_file_for_read(directory: &Dir, name: &OsStr) -> io::Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    refuse_links_and_blocking(&mut options);
    Ok(directory.open_with(name, &options)?.into_std())
}

/// Create one new file beneath an already-open directory for writing.
///
/// A final link is refused, a FIFO is never waited on, and an existing name fails.
pub(crate) fn create_new_file(directory: &Dir, name: &OsStr) -> io::Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    refuse_links_and_blocking(&mut options);
    Ok(directory.open_with(name, &options)?.into_std())
}

/// Rename one file beneath an already-open directory, refusing to replace an existing name.
///
/// `directory_path` is required only by the Windows rename, which is path-based; here the
/// directory handle alone names both sides.
pub(crate) fn rename_without_replacing(
    directory: &Dir,
    _directory_path: &Path,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        use rustix::fs::{RenameFlags, renameat_with};
        use rustix::io::Errno;

        // cap-std's rename always replaces; publication must not.
        match renameat_with(directory, from, directory, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(()),
            Err(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Older kernels retain the same no-clobber contract via linkat.
    // The staged name remains owned by its cleanup guard.
    directory.hard_link(from, directory, to)
}

/// Configure one directory-relative open so it neither follows a final link nor waits on a FIFO.
fn refuse_links_and_blocking(options: &mut cap_std::fs::OpenOptions) {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    use cap_std::fs::OpenOptionsExt;

    options
        .follow(FollowSymlinks::No)
        .mode(0o666)
        .nonblock(true)
        .custom_flags((rustix::fs::OFlags::NOCTTY | rustix::fs::OFlags::CLOEXEC).bits() as i32);
}
