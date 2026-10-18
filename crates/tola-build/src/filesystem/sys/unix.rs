//! Unix: symlink classification, raw-byte path identity, and `rustix` opens.

use std::fs::{File, Permissions};
use std::io;
use std::path::Path;

/// Open one path as a regular file, following or refusing a final symlink.
pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    // NONBLOCK prevents opening a FIFO from waiting for a peer before the type check.
    let mut flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
    if !follow_symlink {
        flags |= OFlags::NOFOLLOW;
    }
    Ok(File::from(open(path, flags, Mode::empty())?))
}

/// Open the persistent lock at one path, creating it without following a final symlink.
pub(in crate::filesystem) fn open_lock_file(path: &Path) -> io::Result<File> {
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
pub(in crate::filesystem) fn set_write_permission(
    permissions: &mut Permissions,
    writable: bool,
    is_dir: bool,
) {
    use std::os::unix::fs::PermissionsExt;

    let mode = permissions.mode();
    permissions.set_mode(if writable {
        mode | if is_dir { 0o700 } else { 0o600 }
    } else {
        mode & !0o222
    });
}

/// The permissions an ordinary file creation has here, when the platform needs them stated.
pub(in crate::filesystem) fn ordinary_file_permissions() -> Option<Permissions> {
    use std::os::unix::fs::PermissionsExt;

    Some(Permissions::from_mode(0o666))
}
