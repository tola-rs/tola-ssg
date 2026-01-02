//! Path and URL utilities.
//!
//! Pure functions for path manipulation. No side effects.
//!
//! - [`fs`]: Filesystem path normalization (`normalize_path`, `resolve_path`)
//! - [`route`]: URL utilities (`is_external_link`, `split_path_fragment`, `url_to_safe_filename`)
//! - [`slug`]: URL slugification (`slugify_path`, `slugify_fragment`)

pub mod fs;
pub mod route;
pub mod slug;

// Re-export commonly used functions from fs (used in many places)
pub use fs::{normalize_path, resolve_path};
