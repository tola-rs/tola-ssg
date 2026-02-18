//! Mtime-based freshness detection for generated files.
//!
//! Used when comparing tola-generated outputs (e.g., HTML -> SVG compression)
//! where timestamps are reliable since both files are created by tola.
//!
//! # When to Use
//!
//! - **mtime**: For comparing tola-generated files (output vs output)
//! - **content-hash**: For source file detection (source vs output)

use std::path::Path;
use std::time::SystemTime;

/// Check if output file is newer than the given source mtime
///
/// Returns `true` if the output exists and is newer than source_mtime,
/// meaning the output is fresh and processing can be skipped
///
/// # Arguments
///
/// * `output` - Path to the output file
/// * `source_mtime` - Modification time of the source file
///
/// # Returns
///
/// `true` if output is fresh (newer than source), `false` otherwise
#[allow(dead_code)] // Reserved for incremental build optimization
pub fn is_output_fresh(output: &Path, source_mtime: Option<SystemTime>) -> bool {
    let Some(source_time) = source_mtime else {
        return false;
    };

    output
        .metadata()
        .and_then(|m| m.modified())
        .map(|output_time| output_time >= source_time)
        .unwrap_or(false)
}

/// Get the modification time of a file
///
/// Returns `None` if the file doesn't exist or mtime cannot be read
pub fn get_mtime(path: &Path) -> Option<SystemTime> {
    path.metadata().and_then(|m| m.modified()).ok()
}

/// Check if file A is newer than file B
///
/// Returns `true` if A exists and is newer than B
/// Returns `false` if either file doesn't exist or times can't be compared
pub fn is_newer_than(a: &Path, b: &Path) -> bool {
    let (Some(a_time), Some(b_time)) = (get_mtime(a), get_mtime(b)) else {
        return false;
    };
    a_time > b_time
}
