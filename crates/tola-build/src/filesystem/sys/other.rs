//! Other targets: standard-library approximations of the platform operations.

use std::fs::File;
use std::io;
use std::path::Path;

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
