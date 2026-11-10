//! Directory replacement with the native exchange or no-replace operation.

use std::path::Path;

use crate::ReplaceDirectoryError;

pub(crate) fn replace_directory(
    source: &Path,
    target: &Path,
    _previous: &Path,
) -> Result<(), ReplaceDirectoryError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    let flags = if target.try_exists()? {
        RenameFlags::EXCHANGE
    } else {
        RenameFlags::NOREPLACE
    };
    Ok(renameat_with(CWD, source, CWD, target, flags).map_err(std::io::Error::from)?)
}
