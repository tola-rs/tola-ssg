//! Windows: share-mode log opens, reparse-point refusals, junction removal, adapted streams,
//! and path-based publication.

use std::ffi::OsStr;
use std::fs::{File, FileType, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use cap_std::fs::Dir;

/// Open one log path for reading.
pub(crate) fn open_log_for_read(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

/// Configure one log open so it shares the file as a log reader and a further appending sink do.
///
/// Windows locks are mandatory: this sharing mode refuses deletion of a log this command writes,
/// while readers and further appending sinks stay compatible with it.
pub(crate) fn open_log_for_append(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
}

/// The number of paths that name one opened file.
pub(crate) fn link_count(file: &File) -> io::Result<u64> {
    Ok(u64::from(
        winapi_util::file::information(file)?.number_of_links(),
    ))
}

/// Create one directory link naming `target`.
pub(crate) fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// Whether this link-like entry must be removed as a directory rather than as a file.
pub(crate) fn link_removes_as_directory(file_type: &FileType) -> bool {
    use std::os::windows::fs::FileTypeExt;

    file_type.is_symlink_dir()
}

/// Write bytes to the process's own standard error, keeping native console/ANSI adaptation.
pub(crate) fn write_process_stderr(bytes: &[u8]) -> io::Result<()> {
    let mut stderr = anstream::AutoStream::always(io::stderr().lock());
    stderr.write_all(bytes)?;
    stderr.flush()
}

/// Open the filesystem root that begins an absolute path, refusing a reparse point.
pub(crate) fn open_filesystem_root(path: &Path) -> io::Result<Dir> {
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
pub(crate) fn open_directory_nofollow(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    use cap_fs_ext::DirExt;

    let directory = parent.open_dir_nofollow(name)?;
    reject_reparse(&directory.dir_metadata()?)?;
    Ok(directory)
}

/// Open one existing file beneath an already-open directory for reading.
///
/// A final reparse point is refused.
pub(crate) fn open_file_for_read(directory: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    open_refusing_reparse(directory, name, &options)
}

/// Create one new file beneath an already-open directory for writing.
///
/// A final reparse point is refused and an existing name fails.
pub(crate) fn create_new_file(directory: &Dir, name: &OsStr) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::OpenOptions;

    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    open_refusing_reparse(directory, name, &options)
}

/// Rename one file beneath an already-open directory, refusing to replace an existing name.
///
/// `MoveFileExW` is path-based, so the pinned `directory_path` is required beside the handle;
/// the Unix implementation renames through its handle alone.
pub(crate) fn rename_without_replacing(
    _directory: &Dir,
    directory_path: &Path,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    // Every ancestor is verified and held without delete sharing.
    // A bare Dir alone would not make this path-based call safe.
    let from: Vec<u16> = directory_path
        .join(from)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let to: Vec<u16> = directory_path
        .join(to)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // No MOVEFILE_REPLACE_EXISTING: a competing save keeps its name.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("write path contains a reparse point"));
    }
    Ok(())
}
