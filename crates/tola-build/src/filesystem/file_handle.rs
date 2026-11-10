//! Native handle opening for persistent lock files.

use std::fs::File;
use std::io;
use std::path::Path;

use tola_fs::is_link_like;

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
    let file = match tola_fs::open_lock_file(path) {
        Ok(file) => file,
        Err(_) if refused_for_path_kind(path) => return Err(OpenLockError::NotRegularFile),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link_like(&metadata) {
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
            if metadata.is_file() && !is_link_like(&metadata) {
                Ok(())
            } else {
                Err(OpenLockError::NotRegularFile)
            }
        }
        // A missing or unreadable path is the open's to report.
        Err(_) => Ok(()),
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use crate::cancellation::BuildCancellation;
    use crate::filesystem::{
        FileLock, FileLockError, TreeObservation, observe_tree_source_selected,
    };
    use tola_input::FilesystemSourceIdentity;

    #[test]
    fn directory_junctions_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("source");
        let target = directory.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("private.txt"), b"outside source").unwrap();
        let link = root.join("linked");
        // Directory junctions do not require the symlink privilege or Developer Mode.
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        let identity = FilesystemSourceIdentity::from_path(&root);
        let cancellation = BuildCancellation::default();
        assert!(
            observe_tree_source_selected(
                TreeObservation {
                    source_identity: &identity,
                    boundary: &tola_typst::SourceBoundary::default(),
                    previous: None,
                    changed_paths: &[],
                    cancellation: &cancellation,
                    display_root: &root,
                    member_label: "source member",
                    validate_member: &|_, _| Ok(()),
                },
                &|_, _| true,
            )
            .is_err()
        );

        let error = FileLock::acquire(&root, &link.join("lock"), &cancellation, || {
            panic!("linked lock directory was accepted")
        })
        .unwrap_err();
        assert!(
            matches!(error, FileLockError::Io(error) if error.kind() == std::io::ErrorKind::InvalidInput)
        );
        assert!(!target.join("lock").exists());
    }
}
