//! The operating system's own behavior for the command's files, directories, and streams,
//! behind one interface.
//!
//! Each operation below has one implementation per platform family, and a caller names the
//! operation rather than the platform: no `cfg` reaches the code that uses it.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{
    create_new_file, link_count, link_directory, link_removes_as_directory,
    open_directory_nofollow, open_file_for_read, open_filesystem_root, open_log_for_append,
    open_log_for_read, rename_without_replacing, write_process_stderr,
};
#[cfg(windows)]
pub(crate) use windows::{
    create_new_file, link_count, link_directory, link_removes_as_directory,
    open_directory_nofollow, open_file_for_read, open_filesystem_root, open_log_for_append,
    open_log_for_read, rename_without_replacing, write_process_stderr,
};
