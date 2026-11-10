//! Native filesystem operations.

use std::ffi::OsStr;
use std::fs::{DirEntry, File, FileType, Metadata, OpenOptions, Permissions};
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

/// Open one log path for reading.
pub fn open_log_for_read(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Configure one log open so it shares the file as a log reader and a further appending sink do.
///
/// Windows locks are mandatory: this sharing mode refuses deletion of a log this command writes,
/// while readers and further appending sinks stay compatible with it.
pub fn open_log_for_append(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
}

/// The number of paths that name one opened file.
pub fn link_count(file: &File) -> io::Result<u64> {
    Ok(winapi_util::file::information(file)?.number_of_links())
}

/// Create one directory link naming `target`.
pub fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// Whether this link-like entry must be removed as a directory rather than as a file.
pub fn link_removes_as_directory(file_type: &FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;

    file_type.is_symlink_dir()
}

/// Open the filesystem root that begins an absolute path, refusing a reparse point.
pub fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    // Ambient cap-std opens follow symlinks. Only the filesystem root may use an
    // ambient path, and even it must reject every reparse tag.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let directory = Dir::from_std_file(file);
    let metadata = directory.dir_metadata()?;
    reject_reparse(&metadata)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("parent is not a directory"));
    }
    Ok(directory)
}

/// Open one directory beneath an already-open parent, refusing a final reparse point.
pub fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    let directory = parent.open_dir_nofollow(name)?;
    reject_reparse(&directory.dir_metadata()?)?;
    Ok(directory)
}

/// Open one existing file beneath an already-open directory for reading.
///
/// A final reparse point is refused.
pub fn open_file_for_read(directory: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    open_refusing_reparse(directory, name, &options)
}

/// Create one new file beneath an already-open directory for writing.
///
/// A final reparse point is refused and an existing name fails.
pub fn create_new_file(directory: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    open_refusing_reparse(directory, name, &options)
}

/// Open one file beneath an already-open directory, refusing a reparse-point handle.
fn open_refusing_reparse(
    directory: &Dir,
    name: &OsStr,
    options: &cap_std::fs::OpenOptions,
) -> io::Result<File> {
    let file = directory.open_with(name, options)?;
    reject_reparse(&file.metadata()?)?;
    Ok(file.into_std())
}

/// Refuse a handle whose attributes name a reparse point, the shape every Windows link takes.
fn reject_reparse(metadata: &cap_std::fs::Metadata) -> io::Result<()> {
    use cap_std::fs::MetadataExt;

    if attributes_are_link_like(metadata.file_attributes()) {
        return Err(io::Error::other("write path contains a reparse point"));
    }
    Ok(())
}

/// Publish a file without replacing a competing name.
///
/// The caller must keep every ancestor of `directory_path` open without delete sharing.
/// The directory handle alone cannot anchor the Windows path-based rename.
pub fn rename_without_replacing(
    _directory: &Dir,
    directory_path: &Path,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    renamore::rename_exclusive(directory_path.join(from), directory_path.join(to))
}

/// Whether this metadata describes a symbolic link or any reparse point.
/// Use entry metadata for path checks and handle metadata for opened-file checks.
pub fn is_link_like(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    attributes_are_link_like(metadata.file_attributes())
}

fn attributes_are_link_like(attributes: u32) -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Classify a directory entry without following it; `file_type` came from this entry.
pub fn entry_is_link_like(entry: &DirEntry, file_type: &FileType) -> io::Result<bool> {
    Ok(file_type.is_symlink() || is_link_like(&std::fs::symlink_metadata(entry.path())?))
}

/// Create one file link naming `target`.
pub fn link_file(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// Read Unix permission bits when this platform has them.
pub fn unix_mode(_: &Metadata) -> Option<u32> {
    None
}

/// Set Unix permission bits; platforms without Unix modes leave permissions unchanged.
pub fn set_unix_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

/// Make one entry writable or read-only, keeping its other permissions.
pub fn set_write_permission(permissions: &mut Permissions, writable: bool, _is_dir: bool) {
    permissions.set_readonly(!writable);
}

/// Explicit ordinary creation permissions, when the platform requires them.
pub fn ordinary_file_permissions() -> Option<Permissions> {
    None
}

/// Encode a filesystem path as a lossless, platform-native hexadecimal identity.
///
/// For equality and ownership markers, not display. Preserves Windows UTF-16 code units.
pub fn encode_path_identity(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    hex::encode(bytes)
}

/// Open the persistent lock at one path, creating it without following a final reparse point.
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

    let mut options = File::options();
    options
        .read(true)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    if !follow_symlink {
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
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
