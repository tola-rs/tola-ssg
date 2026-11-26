//! Staging, replacement, and retention of an unrestored directory.

use std::io;
use std::path::{Path, PathBuf};

/// Staged directory beside the destination it will replace.
///
/// Dropping removes the staging workspace unless restoration failed, in which case the previous
/// directory is retained and returned via [`ReplaceDirectoryError::Restore`].
pub struct DirectoryReplacement {
    workspace: tempfile::TempDir,
    target: PathBuf,
}

impl DirectoryReplacement {
    /// Prepare a workspace on the destination's filesystem beneath its existing parent.
    pub fn new(target: &Path) -> io::Result<Self> {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination needs a parent directory",
                )
            })?;
        Ok(Self {
            workspace: tempfile::tempdir_in(parent)?,
            target: target.to_owned(),
        })
    }

    /// Temporary workspace path (callers must exclude it from source enumeration).
    pub fn workspace(&self) -> &Path {
        self.workspace.path()
    }

    /// Path where the caller builds and validates the replacement directory.
    pub fn staged(&self) -> PathBuf {
        self.workspace.path().join("staged")
    }

    /// Install the staged directory and discard the old one on success.
    pub fn publish(self) -> Result<(), ReplaceDirectoryError> {
        let res = super::sys::replace_directory(
            &self.staged(),
            &self.target,
            &self.workspace.path().join("previous"),
        );
        if matches!(&res, Err(ReplaceDirectoryError::Restore { .. })) {
            let _ = self.workspace.keep();
        }
        res
    }
}

/// Error replacing a destination directory.
#[derive(Debug, thiserror::Error)]
pub enum ReplaceDirectoryError {
    /// Installation failed before replacement, or the old directory was restored.
    #[error("could not replace the directory")]
    Io(#[from] io::Error),
    /// Installation and restoration both failed; the old directory remains at `previous`.
    #[error("could not restore the previous directory")]
    Restore {
        /// Restoration failure.
        #[source]
        source: io::Error,
        /// Retained old directory outside temporary cleanup.
        previous: PathBuf,
    },
}
