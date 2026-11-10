//! Native filesystem operations shared by Tola's readers and writers.
//!
//! Checks on opened files inspect their handles. Path observations never prove that a later
//! open sees the same entry. Callers own selection, publication timing, and author diagnostics.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io;
use std::path::Path;

mod replace;
mod sys;

pub use replace::{DirectoryReplacement, ReplaceDirectoryError};
pub use sys::{
    create_new_file, encode_path_identity, entry_is_link_like, is_link_like, link_count,
    link_directory, link_file, link_removes_as_directory, open_directory_nofollow,
    open_file_for_read, open_filesystem_root, open_lock_file, open_log_for_append,
    open_log_for_read, ordinary_file_permissions, rename_without_replacing, set_unix_mode,
    set_write_permission, unix_mode,
};

/// Open a regular file for reading, following or refusing a final link.
///
/// Unix opens do not wait on FIFOs. Windows refuses device handles and, when following is
/// disabled, every reparse point. Intermediate directories are not pinned by this operation.
pub fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    let file = sys::open_regular_file(path, follow_symlink)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn regular_inputs_are_read_only() {
        use std::io::Write;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.css");
        std::fs::write(&path, b"original input").unwrap();
        let mut file = open_regular_file(&path, false).unwrap();
        assert!(file.write_all(b"replacement").is_err());
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original input");
    }

    #[cfg(unix)]
    #[test]
    fn links_require_explicit_following() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.css");
        let link = directory.path().join("linked.css");
        std::fs::write(&target, b"linked input").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(open_regular_file(&link, false).is_err());
        let mut file = open_regular_file(&link, true).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"linked input");
    }

    #[cfg(windows)]
    #[test]
    fn device_handles_are_rejected() {
        let error = open_regular_file(Path::new("NUL"), true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
