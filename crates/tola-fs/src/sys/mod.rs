//! Platform selection for filesystem operations.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
mod exchange;
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(crate) use exchange::replace_directory;
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
mod backup;
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(crate) use backup::replace_directory;

#[cfg(not(any(unix, windows)))]
compile_error!("tola-fs requires a Unix or Windows target");
