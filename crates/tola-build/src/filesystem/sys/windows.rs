//! Windows: reparse-point classification, UTF-16 path identity, and handle opens.

use std::fs::{File, FileType, Permissions};
use std::io;
use std::path::Path;

/// Whether the entry at one observed path is a link-like entry.
pub(crate) fn automatic_path_is_link_like(path: &Path, file_type: &FileType) -> io::Result<bool> {
    if file_type.is_symlink() {
        return Ok(true);
    }
    // Path validation inspects the current directory entry, not a followed target.
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(metadata_is_link_like(&metadata))
}

/// Whether handle metadata names a reparse point, the shape every Windows link takes.
fn metadata_is_link_like(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    // Safe-open callers supply handle metadata so a renamed path cannot rebind this check.
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Encode a filesystem path as a lossless, platform-native hexadecimal identity.
///
/// For equality and ownership markers, not display. Preserves Windows UTF-16 code units.
pub(crate) fn encode_path_identity(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    hex::encode(bytes)
}

/// Open one path as a regular file, following or refusing a final reparse point.
pub(crate) fn open_regular_file(path: &Path, follow_symlink: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
    };

    let mut options = File::options();
    options
        .read(true)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    if !follow_symlink {
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !winapi_util::file::typ(&file)?.is_disk() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is not a disk file",
        ));
    }
    if !follow_symlink && metadata_is_link_like(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "this path is a reparse point",
        ));
    }
    Ok(file)
}

/// Open the persistent lock at one path, creating it without following a final reparse point.
pub(in crate::filesystem) fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, SECURITY_IDENTIFICATION,
    };

    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)
}

/// Make one entry writable or read-only.
pub(in crate::filesystem) fn set_write_permission(
    permissions: &mut Permissions,
    writable: bool,
    _is_dir: bool,
) {
    permissions.set_readonly(!writable);
}

/// The permissions an ordinary file creation has here, when the platform needs them stated.
pub(in crate::filesystem) fn ordinary_file_permissions() -> Option<Permissions> {
    None
}

#[cfg(test)]
mod tests {
    use crate::cancellation::BuildCancellation;
    use crate::filesystem::{
        FileLock, FileLockError, FilesystemSourceIdentity, observe_tree_source,
    };

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
            observe_tree_source(
                &identity,
                &tola_typst::SourceBoundary::default(),
                None,
                &[],
                &cancellation,
                &root,
                "source member",
                &|_, _| Ok(()),
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
