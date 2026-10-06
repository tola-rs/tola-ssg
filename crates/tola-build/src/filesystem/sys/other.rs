//! Other targets: standard-library approximations of the platform operations.

use std::fs::{File, FileType, Permissions};
use std::io;
use std::path::Path;

/// Whether the entry at one observed path is a link-like entry.
pub(crate) fn automatic_path_is_link_like(_path: &Path, file_type: &FileType) -> io::Result<bool> {
    Ok(file_type.is_symlink())
}

/// Encode a filesystem path as a lossless hexadecimal identity.
pub(crate) fn encode_path_identity(path: &Path) -> String {
    hex::encode(path.as_os_str().as_encoded_bytes())
}

/// Open one path as a regular file, following or refusing a final symlink.
pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    if !follow_symlink && std::fs::symlink_metadata(path)?.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is a symbolic link",
        ));
    }
    File::open(path)
}

/// Open the persistent lock at one path, creating it.
pub(in crate::filesystem) fn open_lock_file(path: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Make one entry writable or read-only.
pub(in crate::filesystem) fn set_write_permission(
    permissions: &mut Permissions,
    writable: bool,
    _is_dir: bool,
) {
    permissions.set_readonly(!writable);
}

/// The permissions an ordinary file creation has here, when the platform needs them stated.
pub(in crate::filesystem) fn ordinary_file_permissions() -> Option<Permissions> {
    None
}
