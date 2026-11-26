//! Native Unix filesystem operations.

use std::ffi::OsStr;
use std::fs::{self, DirEntry, File, FileType, Metadata, OpenOptions, Permissions};
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

/// Open a log file for reading without following links or waiting on a FIFO.
pub fn open_log_for_read(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    refuse_links_and_fifos(&mut opts);
    opts.open(path)
}

/// Configure a log open with `0o600` mode, `NOFOLLOW`, and `NONBLOCK`.
pub fn open_log_for_append(opts: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    opts.mode(0o600);
    refuse_links_and_fifos(opts);
}

/// Number of directory entries pointing to `file`.
pub fn link_count(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    Ok(file.metadata()?.nlink())
}

/// Create a directory symlink at `link` pointing to `target`.
pub fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Unix removes all symlinks as files, not directories.
pub fn link_removes_as_directory(_ft: &FileType) -> bool {
    false
}

/// Open the filesystem root beginning an absolute path.
pub fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

/// Open a child directory beneath `parent`, refusing a final symlink.
pub fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    parent.open_dir_nofollow(name)
}

/// Open an existing file beneath `dir` for reading without following links or waiting on a FIFO.
pub fn open_file_for_read(dir: &Dir, name: &OsStr) -> io::Result<File> {
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.read(true);
    refuse_links_and_blocking(&mut opts);
    Ok(dir.open_with(name, &opts)?.into_std())
}

/// Create a new file beneath `dir` for writing (`O_CREAT | O_EXCL`, no links/FIFOs).
pub fn create_new_file(dir: &Dir, name: &OsStr) -> io::Result<File> {
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    refuse_links_and_blocking(&mut opts);
    Ok(dir.open_with(name, &opts)?.into_std())
}

fn refuse_links_and_fifos(opts: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    opts.custom_flags((rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32);
}

/// Rename `from` to `to` beneath `dir` without replacing an existing entry.
pub fn rename_without_replacing(
    dir: &Dir,
    _dir_path: &Path,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        use rustix::fs::{RenameFlags, renameat_with};
        use rustix::io::Errno;

        // cap-std's rename always replaces; publication must not.
        match renameat_with(dir, from, dir, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(()),
            Err(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => {}
            Err(err) => return Err(err.into()),
        }
    }
    // Older kernels retain the no-clobber contract via linkat; caller's guard cleans up `from`.
    dir.hard_link(from, dir, to)
}

fn refuse_links_and_blocking(opts: &mut cap_std::fs::OpenOptions) {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    use cap_std::fs::OpenOptionsExt;

    opts.follow(FollowSymlinks::No)
        .mode(0o666)
        .nonblock(true)
        .custom_flags((rustix::fs::OFlags::NOCTTY | rustix::fs::OFlags::CLOEXEC).bits() as i32);
}

/// Return whether `meta` describes a symbolic link.
pub fn is_link_like(meta: &Metadata) -> bool {
    meta.is_symlink()
}

/// Classify a directory entry without following it.
pub fn entry_is_link_like(_entry: &DirEntry, ft: &FileType) -> io::Result<bool> {
    Ok(ft.is_symlink())
}

/// Create a file symlink at `link` pointing to `target`.
pub fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Return Unix permission mode bits.
pub fn unix_mode(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.mode())
}

/// Apply Unix permission mode bits to `path`.
pub fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, Permissions::from_mode(mode))
}

/// Lossless hex encoding of `path` bytes for equality and ownership records.
pub fn encode_path_identity(path: &Path) -> String {
    hex::encode(path.as_os_str().as_encoded_bytes())
}

/// Open or create the lock file at `path` without following a final symlink.
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

/// Set or clear write bits while preserving other mode bits.
pub fn set_write_permission(perms: &mut Permissions, writable: bool, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;

    let mode = perms.mode();
    perms.set_mode(if writable {
        mode | if is_dir { 0o700 } else { 0o600 }
    } else {
        mode & !0o222
    });
}

/// Default creation permissions for ordinary files (`0o666`).
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
