//! Native Windows filesystem operations.

use std::ffi::OsStr;
use std::fs::{DirEntry, File, FileType, Metadata, OpenOptions, Permissions};
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

/// Open a log file for reading.
pub fn open_log_for_read(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Share read/write access on a log file while refusing deletion (mandatory Windows locking).
pub fn open_log_for_append(opts: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
}

/// Number of directory entries pointing to `file`.
pub fn link_count(file: &File) -> io::Result<u64> {
    Ok(winapi_util::file::information(file)?.number_of_links())
}

/// Create a directory symlink at `link` pointing to `target`.
pub fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// Return whether this link-like entry must be removed as a directory.
pub fn link_removes_as_directory(ft: &FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;

    ft.is_symlink_dir()
}

/// Open the filesystem root beginning an absolute path, refusing any reparse point.
pub fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    // Ambient cap-std opens follow symlinks; open the root handle directly and reject reparse tags.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let dir = Dir::from_std_file(file);
    let meta = dir.dir_metadata()?;
    reject_reparse(&meta)?;
    if !meta.is_dir() {
        return Err(io::Error::other("parent is not a directory"));
    }
    Ok(dir)
}

/// Open a child directory beneath `parent`, refusing a final reparse point.
pub fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    let dir = parent.open_dir_nofollow(name)?;
    reject_reparse(&dir.dir_metadata()?)?;
    Ok(dir)
}

/// Open an existing file beneath `dir` for reading, refusing a final reparse point.
pub fn open_file_for_read(dir: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut opts = OpenOptions::new();
    opts.read(true).follow(FollowSymlinks::No);
    open_refusing_reparse(dir, name, &opts)
}

/// Create a new file beneath `dir` for writing, refusing a final reparse point.
pub fn create_new_file(dir: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true).follow(FollowSymlinks::No);
    open_refusing_reparse(dir, name, &opts)
}

fn open_refusing_reparse(
    dir: &Dir,
    name: &OsStr,
    opts: &cap_std::fs::OpenOptions,
) -> io::Result<File> {
    let file = dir.open_with(name, opts)?;
    reject_reparse(&file.metadata()?)?;
    Ok(file.into_std())
}

fn reject_reparse(meta: &cap_std::fs::Metadata) -> io::Result<()> {
    use cap_std::fs::MetadataExt;

    if attrs_are_link_like(meta.file_attributes()) {
        return Err(io::Error::other("write path contains a reparse point"));
    }
    Ok(())
}

/// Rename `from` to `to` beneath `dir_path` without replacing an existing entry.
///
/// Caller must keep every ancestor of `dir_path` open without delete sharing because the Windows
/// rename API is path-based.
pub fn rename_without_replacing(
    _dir: &Dir,
    dir_path: &Path,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    renamore::rename_exclusive(dir_path.join(from), dir_path.join(to))
}

/// Return whether `meta` describes a symbolic link or reparse point.
pub fn is_link_like(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    attrs_are_link_like(meta.file_attributes())
}

fn attrs_are_link_like(attrs: u32) -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Classify a directory entry without following it.
pub fn entry_is_link_like(entry: &DirEntry, ft: &FileType) -> io::Result<bool> {
    Ok(ft.is_symlink() || is_link_like(&std::fs::symlink_metadata(entry.path())?))
}

/// Create a file symlink at `link` pointing to `target`.
pub fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// No Unix permission bits on Windows.
pub fn unix_mode(_: &Metadata) -> Option<u32> {
    None
}

/// No-op on Windows.
pub fn set_unix_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

/// Toggle the read-only attribute while preserving other attributes.
pub fn set_write_permission(perms: &mut Permissions, writable: bool, _is_dir: bool) {
    perms.set_readonly(!writable);
}

/// No explicit creation permissions required on Windows.
pub fn ordinary_file_permissions() -> Option<Permissions> {
    None
}

/// Lossless hex encoding of `path` UTF-16 code units for equality and ownership records.
pub fn encode_path_identity(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    hex::encode(bytes)
}

/// Open or create the lock file at `path` without following a final reparse point.
pub fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
    };

    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)
}

pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
    };

    let mut opts = File::options();
    opts.read(true).security_qos_flags(SECURITY_IDENTIFICATION);
    if !follow_symlink {
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = opts.open(path)?;
    if !winapi_util::file::typ(&file)?.is_disk() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is not a disk file",
        ));
    }
    if !follow_symlink && is_link_like(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is a reparse point",
        ));
    }
    Ok(file)
}
