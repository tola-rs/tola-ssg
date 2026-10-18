//! The operating system's own filesystem behavior, behind one interface.
//!
//! Each operation below has one implementation per platform family, and a caller names the
//! operation rather than the platform: no `cfg` reaches the filesystem code that uses it.

#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(not(any(unix, windows)))]
pub(super) use other::open_lock_file;
#[cfg(not(any(unix, windows)))]
pub(crate) use other::open_regular_file;
#[cfg(unix)]
pub(crate) use unix::open_regular_file;
#[cfg(unix)]
pub(super) use unix::{open_lock_file, ordinary_file_permissions, set_write_permission};
#[cfg(windows)]
pub(super) use windows::open_lock_file;
#[cfg(windows)]
pub(crate) use windows::{automatic_path_is_link_like, encode_path_identity, open_regular_file};

/// Windows and other targets state writability through the readonly bit alone.
#[cfg(not(unix))]
pub(in crate::filesystem) fn set_write_permission(
    permissions: &mut std::fs::Permissions,
    writable: bool,
    _is_dir: bool,
) {
    permissions.set_readonly(!writable);
}

#[cfg(not(unix))]
pub(in crate::filesystem) fn ordinary_file_permissions() -> Option<std::fs::Permissions> {
    None
}

/// Unix and other targets classify a link by the entry type alone.
#[cfg(not(windows))]
pub(crate) fn automatic_path_is_link_like(
    _path: &std::path::Path,
    file_type: &std::fs::FileType,
) -> std::io::Result<bool> {
    Ok(file_type.is_symlink())
}

/// Path identity for ownership markers, raw bytes on Unix and other targets.
#[cfg(not(windows))]
pub(crate) fn encode_path_identity(path: &std::path::Path) -> String {
    hex::encode(path.as_os_str().as_encoded_bytes())
}
