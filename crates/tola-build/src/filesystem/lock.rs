//! Cancellable operating-system file locks beneath a checked directory root.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path};
use std::time::Duration;

use crate::cancellation::{BuildCancellation, BuildCancelled};

use super::file_handle::{OpenLockError, open_lock_file};
use tola_fs::is_link_like;

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileLockError {
    #[error("{0}")]
    Cancelled(#[from] BuildCancelled),
    /// The path is not a regular file, so no process can hold this lock.
    #[error("the lock path must be a regular file")]
    NotRegularFile,
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl From<OpenLockError> for FileLockError {
    fn from(error: OpenLockError) -> Self {
        match error {
            OpenLockError::NotRegularFile => Self::NotRegularFile,
            OpenLockError::Io(error) => Self::Io(error),
        }
    }
}

/// Closing the sole handle releases the lock, including after process termination.
/// The pathname remains: unlinking it could let another process lock a different file.
#[derive(Debug)]
pub(crate) struct FileLock {
    _file: File,
}

impl FileLock {
    /// Wait for the lock at `path`, creating that pathname only when it is absent.
    ///
    /// A path that is not a regular file cannot hold this lock and is reported as
    /// [`FileLockError::NotRegularFile`]; any other filesystem failure keeps its own error.
    /// Cancelling the wait is [`FileLockError::Cancelled`].
    pub(crate) fn acquire(
        root: &Path,
        path: &Path,
        cancellation: &BuildCancellation,
        on_wait: impl FnOnce(),
    ) -> Result<Self, FileLockError> {
        cancellation.ensure_active()?;
        create_lock_parent(root, path)?;
        let file = open_lock_file(path)?;
        let mut on_wait = Some(on_wait);
        loop {
            cancellation.ensure_active()?;
            match file.try_lock() {
                Ok(()) => {
                    cancellation.ensure_active()?;
                    return Ok(Self { _file: file });
                }
                Err(fs::TryLockError::WouldBlock) => {
                    if let Some(on_wait) = on_wait.take() {
                        on_wait();
                    }
                    cancellation.ensure_active()?;
                    std::thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}

fn create_lock_parent(root: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "lock must stay inside its directory root",
        )
    })?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid relative lock path",
        ));
    }
    let mut directory = root.to_path_buf();
    for part in relative
        .parent()
        .expect("lock has a relative parent")
        .components()
    {
        directory.push(part.as_os_str());
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || is_link_like(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "lock directory `{}` is a symbolic link or not a directory; replace it with a real directory",
                    tola_diagnostic::display_path(&directory, root)
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_preserves_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        fs::write(&path, b"persistent lock contents").unwrap();

        let lock = FileLock::acquire(dir.path(), &path, &BuildCancellation::default(), || {
            panic!("uncontended lock waited")
        })
        .unwrap();

        // Windows locks the byte range; drop before reading back.
        drop(lock);
        assert_eq!(fs::read(&path).unwrap(), b"persistent lock contents");
    }

    #[cfg(unix)]
    #[test]
    fn lock_permissions_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/lock");
        let _lock = FileLock::acquire(dir.path(), &path, &BuildCancellation::default(), || {
            panic!("new lock waited")
        })
        .unwrap();

        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
    }

    #[test]
    fn lock_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        fs::create_dir(&path).unwrap();

        let err = FileLock::acquire(dir.path(), &path, &BuildCancellation::default(), || {
            panic!("an unusable lock path waited")
        })
        .unwrap_err();

        assert!(matches!(err, FileLockError::NotRegularFile));
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn lock_rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        let outside = dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let target = outside.join("existing");
        fs::write(&target, b"unrelated contents").unwrap();
        let linked_file = root.join("lock");
        std::os::unix::fs::symlink(&target, &linked_file).unwrap();
        let cancel = BuildCancellation::default();

        let err = FileLock::acquire(&root, &linked_file, &cancel, || {
            panic!("linked lock file was accepted")
        })
        .unwrap_err();
        assert!(matches!(err, FileLockError::NotRegularFile));
        assert_eq!(fs::read(&target).unwrap(), b"unrelated contents");

        let linked_dir = root.join("linked");
        std::os::unix::fs::symlink(&outside, &linked_dir).unwrap();
        let err = FileLock::acquire(&root, &linked_dir.join("lock"), &cancel, || {
            panic!("linked lock directory was accepted")
        })
        .unwrap_err();
        assert!(matches!(err, FileLockError::Io(e) if e.kind() == io::ErrorKind::InvalidInput));
        assert!(!outside.join("lock").exists());
    }
}
