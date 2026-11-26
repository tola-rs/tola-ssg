//! Directory replacement on platforms without a native exchange.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::ReplaceDirectoryError;

pub(crate) fn replace_directory(
    src: &Path,
    dst: &Path,
    prev: &Path,
) -> Result<(), ReplaceDirectoryError> {
    let replaced = match fs::rename(dst, prev) {
        Ok(()) => true,
        Err(err) if err.kind() == ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    if let Err(err) = fs::rename(src, dst) {
        if replaced {
            fs::rename(prev, dst).map_err(|source| ReplaceDirectoryError::Restore {
                source,
                previous: prev.to_owned(),
            })?;
        }
        return Err(err.into());
    }
    Ok(())
}
