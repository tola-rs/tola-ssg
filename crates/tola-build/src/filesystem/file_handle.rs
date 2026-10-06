//! Native handle opening for regular inputs and persistent lock files.

use std::fs::File;
use std::io;
use std::path::Path;

use super::sys::automatic_path_is_link_like;

pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    let file = super::sys::open_regular_file(path, follow_symlink)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is not a regular file",
        ));
    }
    Ok(file)
}

/// Why the persistent lock at one path could not be opened.
#[derive(Debug, thiserror::Error)]
pub(super) enum OpenLockError {
    /// The path is not a regular file, so no process can hold this lock.
    #[error("the lock path must be a regular file")]
    NotRegularFile,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Open the persistent lock at `path`, creating it when it is absent.
///
/// The path's own kind is checked before the open, because opening a directory for writing is an
/// access error on some platforms.
pub(super) fn open_lock_file(path: &Path) -> Result<File, OpenLockError> {
    reject_non_regular_lock_path(path)?;
    // The open stays authoritative: another process can replace the path in between.
    let file = match super::sys::open_lock_file(path) {
        Ok(file) => file,
        Err(_) if refused_for_path_kind(path) => return Err(OpenLockError::NotRegularFile),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || automatic_path_is_link_like(path, &metadata.file_type())? {
        return Err(OpenLockError::NotRegularFile);
    }
    Ok(file)
}

/// Whether the path a failed open names is one no lock can take.
///
/// The final component is opened without following links, so a link or directory that appears
/// there under the earlier check fails for the path itself rather than for anything it names.
/// Every other failure stays an [`OpenLockError::Io`], because a regular file can still be
/// refused by its filesystem.
fn refused_for_path_kind(path: &Path) -> bool {
    matches!(
        reject_non_regular_lock_path(path),
        Err(OpenLockError::NotRegularFile)
    )
}

fn reject_non_regular_lock_path(path: &Path) -> Result<(), OpenLockError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.is_file() && !automatic_path_is_link_like(path, &metadata.file_type())? {
                Ok(())
            } else {
                Err(OpenLockError::NotRegularFile)
            }
        }
        // A missing or unreadable path is the open's to report.
        Err(_) => Ok(()),
    }
}
