//! Directory replacement on platforms without a native exchange.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::ReplaceDirectoryError;

pub(crate) fn replace_directory(
    source: &Path,
    target: &Path,
    previous: &Path,
) -> Result<(), ReplaceDirectoryError> {
    let replaced = match fs::rename(target, previous) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if let Err(error) = fs::rename(source, target) {
        if replaced {
            fs::rename(previous, target).map_err(|source| ReplaceDirectoryError::Restore {
                source,
                previous: previous.to_owned(),
            })?;
        }
        return Err(error.into());
    }
    Ok(())
}
