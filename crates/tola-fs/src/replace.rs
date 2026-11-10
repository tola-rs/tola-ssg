//! Staging, replacement, and retention of an unrestored directory.

use std::io;
use std::path::{Path, PathBuf};

/// A staged directory beside the destination it will replace.
///
/// Dropping this value removes its workspace. A failed restoration retains the old directory
/// and returns its location instead; the caller decides how to present that recovery action.
pub struct DirectoryReplacement {
    workspace: tempfile::TempDir,
    target: PathBuf,
}

impl DirectoryReplacement {
    /// Prepare a workspace on the destination's filesystem, beneath its existing parent.
    pub fn new(target: &Path) -> io::Result<Self> {
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
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

    /// The whole temporary workspace, which source enumeration must exclude.
    pub fn workspace(&self) -> &Path {
        self.workspace.path()
    }

    /// Where the caller creates and validates the complete replacement directory.
    pub fn staged(&self) -> PathBuf {
        self.workspace.path().join("staged")
    }

    /// Install the staged directory and dispose of the old one on success.
    ///
    /// Native exchange is used where available. Other platforms move the old directory aside
    /// and restore it if installation fails; that sequence is not an atomic exchange.
    pub fn publish(self) -> Result<(), ReplaceDirectoryError> {
        let result = super::sys::replace_directory(
            &self.staged(),
            &self.target,
            &self.workspace.path().join("previous"),
        );
        if matches!(&result, Err(ReplaceDirectoryError::Restore { .. })) {
            let _ = self.workspace.keep();
        }
        result
    }
}

/// Why a staged directory could not replace its destination.
#[derive(Debug, thiserror::Error)]
pub enum ReplaceDirectoryError {
    /// Installation failed before replacement, or the old directory was restored.
    #[error("could not replace the directory")]
    Io(#[from] io::Error),
    /// Installation and restoration failed; the old directory remains at `previous`.
    #[error("could not restore the previous directory")]
    Restore {
        /// The restoration failure.
        #[source]
        source: io::Error,
        /// The retained old directory, outside automatic temporary cleanup.
        previous: PathBuf,
    },
}
