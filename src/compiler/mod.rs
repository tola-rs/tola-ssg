//! Compilation and asset processing for static site generation.
//!
//! This module orchestrates the build pipeline:
//!
//! - **pages**: Compile `.typ` files to HTML
//! - **meta**: Extract and process page metadata
//! - **assets**: Copy and optimize static assets
//! - **watch**: Incremental builds on file changes
//! - **deps**: Dependency tracking for precise rebuilds
//!
//! # Build Flow
//!
//! ```text
//! collect_pages() ──► compile_pages() ──► process_asset()
//!       │                   │                  │
//!       ▼                   ▼                  ▼
//!   PageMeta[]         HTML files        Asset files
//! ```

pub mod assets;
pub mod deps;
pub mod meta;
pub mod pages;
pub mod watch;

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

// ============================================================================
// Public API
// ============================================================================

pub use assets::process_asset;
pub use assets::process_rel_asset;
pub use pages::collect_metadata;
pub use pages::compile_pages_with_data;
pub use watch::process_watched_files;

// Legacy single-phase API (kept for potential future use)
#[allow(unused_imports)]
pub use pages::collect_pages;
#[allow(unused_imports)]
pub use pages::compile_pages;

// ============================================================================
// Shared utilities
// ============================================================================

/// Files to ignore during directory traversal
const IGNORED_FILES: &[&str] = &[".DS_Store"];

/// Collect all files from a directory recursively.
pub fn collect_all_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            !IGNORED_FILES.contains(&name)
        })
        .map(walkdir::DirEntry::into_path)
        .collect()
}

/// Check if destination is up-to-date compared to source and dependencies.
pub fn is_up_to_date(src: &Path, dst: &Path, deps_mtime: Option<SystemTime>) -> bool {
    let Ok(src_meta) = src.metadata() else {
        return false;
    };
    let Ok(dst_meta) = dst.metadata() else {
        return false;
    };

    let Ok(src_time) = src_meta.modified() else {
        return false;
    };
    let Ok(dst_time) = dst_meta.modified() else {
        return false;
    };

    // Check if source is newer than destination
    if src_time > dst_time {
        return false;
    }

    // Check if any dependency is newer than destination
    if let Some(deps) = deps_mtime
        && deps > dst_time
    {
        return false;
    }

    true
}
