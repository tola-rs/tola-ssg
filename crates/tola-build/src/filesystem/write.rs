//! Complete-file replacement using owned temporary files in the target directory.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Replace a file after its complete contents have been written.
///
/// The temporary file uses ordinary file-creation permissions, subject to the
/// process umask on Unix. This does not synchronize the file or parent directory
/// to durable storage, and is not a directory-anchored site-write transaction.
pub fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new();
    temporary.prefix(".tola-").suffix(".tmp");
    if let Some(permissions) = super::sys::ordinary_file_permissions() {
        temporary.permissions(permissions);
    }
    let mut file = temporary.tempfile_in(parent)?;
    file.write_all(contents)?;
    file.persist(path).map(drop).map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_publishes_ordinary_mode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/output.json");
        atomic_write(&path, b"old contents").unwrap();
        atomic_write(&path, b"replacement contents").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"replacement contents");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ordinary = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.path().join("ordinary"))
                .unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                ordinary.metadata().unwrap().permissions().mode() & 0o777,
            );
        }
    }

    #[test]
    fn failed_publication_leaves_only_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("output");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("retained"), b"previous output").unwrap();

        assert!(atomic_write(&target, b"unpublished").is_err());
        assert_eq!(
            fs::read(target.join("retained")).unwrap(),
            b"previous output"
        );
        let entries = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![target]);
    }
}
