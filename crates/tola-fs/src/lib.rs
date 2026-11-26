//! Native filesystem operations shared by Tola's readers and writers.
//!
//! Handle checks inspect the opened descriptor; callers own selection, timing, and diagnostics.

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
/// Unix avoids blocking on FIFOs; Windows refuses device handles and (when unfollowed) reparse points.
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
    use std::io::{Read, Write};

    #[test]
    fn read_only_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.css");
        std::fs::write(&path, b"original input").unwrap();
        let mut file = open_regular_file(&path, false).unwrap();
        assert!(file.write_all(b"replacement").is_err());
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"original input");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_follow_flag() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.css");
        let link = dir.path().join("linked.css");
        std::fs::write(&target, b"linked input").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(open_regular_file(&link, false).is_err());
        let mut file = open_regular_file(&link, true).unwrap();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"linked input");
    }

    #[cfg(windows)]
    #[test]
    fn reject_device_handle() {
        let err = open_regular_file(Path::new("NUL"), true).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
