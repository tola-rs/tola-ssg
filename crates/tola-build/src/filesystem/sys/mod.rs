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
pub(crate) use other::{automatic_path_is_link_like, encode_path_identity, open_regular_file};
#[cfg(not(any(unix, windows)))]
pub(super) use other::{open_lock_file, ordinary_file_permissions, set_write_permission};
#[cfg(unix)]
pub(crate) use unix::{automatic_path_is_link_like, encode_path_identity, open_regular_file};
#[cfg(unix)]
pub(super) use unix::{open_lock_file, ordinary_file_permissions, set_write_permission};
#[cfg(windows)]
pub(crate) use windows::{automatic_path_is_link_like, encode_path_identity, open_regular_file};
#[cfg(windows)]
pub(super) use windows::{open_lock_file, ordinary_file_permissions, set_write_permission};
