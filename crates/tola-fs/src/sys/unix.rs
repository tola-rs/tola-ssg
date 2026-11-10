//! Native filesystem operations.

use std::ffi::OsStr;
use std::fs::{self, DirEntry, File, FileType, Metadata, OpenOptions, Permissions};
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

/// Open one log path for reading without following links or waiting on a FIFO.
pub fn open_log_for_read(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    refuse_links_and_fifos(&mut options);
    options.open(path)
}

/// Configure one log open so it neither blocks on a raced FIFO nor follows a replacement link.
pub fn open_log_for_append(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
    refuse_links_and_fifos(options);
}

/// The number of paths that name one opened file.
pub fn link_count(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    Ok(file.metadata()?.nlink())
}

/// Create one directory link naming `target`.
pub fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Whether this link-like entry must be removed as a directory rather than as a file.
pub fn link_removes_as_directory(_file_type: &FileType) -> bool {
    false
}

/// Open the filesystem root that begins an absolute path.
pub fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

/// Open one directory beneath an already-open parent, refusing a final link.
pub fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    parent.open_dir_nofollow(name)
}

/// Open one existing file beneath an already-open directory for reading.
///
/// A final link is refused and a FIFO is never waited on.
pub fn open_file_for_read(directory: &Dir, name: &OsStr) -> io::Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    refuse_links_and_blocking(&mut options);
    Ok(directory.open_with(name, &options)?.into_std())
}

/// Create one new file beneath an already-open directory for writing.
///
/// A final link is refused, a FIFO is never waited on, and an existing name fails.
pub fn create_new_file(directory: &Dir, name: &OsStr) -> io::Result<File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    refuse_links_and_blocking(&mut options);
    Ok(directory.open_with(name, &options)?.into_std())
}

fn refuse_links_and_fifos(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32);
}

/// Rename one file beneath an already-open directory, refusing to replace an existing name.
///
/// `directory_path` is required only by the Windows rename, which is path-based; here the
/// directory handle alone names both sides.
pub fn rename_without_replacing(
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

/// Whether this metadata describes a symbolic link.
/// Use entry metadata for path checks and handle metadata for opened-file checks.
pub fn is_link_like(metadata: &Metadata) -> bool {
    metadata.is_symlink()
}

/// Classify a directory entry without following it; `file_type` came from this entry.
pub fn entry_is_link_like(_entry: &DirEntry, file_type: &FileType) -> io::Result<bool> {
    Ok(file_type.is_symlink())
}

/// Create one file link naming `target`.
pub fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Read Unix permission bits when this platform has them.
pub fn unix_mode(metadata: &Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode())
}

/// Set Unix permission bits; platforms without Unix modes leave permissions unchanged.
pub fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, Permissions::from_mode(mode))
}

/// Encode a path losslessly for equality and ownership records, not display.
pub fn encode_path_identity(path: &Path) -> String {
    hex::encode(path.as_os_str().as_encoded_bytes())
}

/// Open the persistent lock at one path, creating it without following a final symlink.
pub fn open_lock_file(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    Ok(File::from(open(
        path,
        OFlags::RDWR
            | OFlags::CREATE
            | OFlags::CLOEXEC
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::NOCTTY,
        Mode::RUSR | Mode::WUSR,
    )?))
}

/// Make one entry writable or read-only, keeping its other mode bits.
pub fn set_write_permission(permissions: &mut Permissions, writable: bool, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;

    let mode = permissions.mode();
    permissions.set_mode(if writable {
        mode | if is_dir { 0o700 } else { 0o600 }
    } else {
        mode & !0o222
    });
}

/// The permissions an ordinary file creation has here, when the platform needs them stated.
pub fn ordinary_file_permissions() -> Option<Permissions> {
    use std::os::unix::fs::PermissionsExt;

    Some(Permissions::from_mode(0o666))
}

pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    // NONBLOCK prevents opening a FIFO from waiting for a peer before the type check.
    let mut flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
    if !follow_symlink {
        flags |= OFlags::NOFOLLOW;
    }
    Ok(File::from(open(path, flags, Mode::empty())?))
}
