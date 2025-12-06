//! Global shared package storage.
//!
//! Package downloads and caching are shared across all compilations to avoid
//! redundant network requests and disk I/O.
//!
//! # Package System Overview
//!
//! Typst packages are referenced in documents like:
//! ```typst
//! #import "@preview/cetz:0.3.0": canvas, draw
//! ```
//!
//! When a package is imported:
//! 1. Check if it exists in the local cache
//! 2. If not, download from the Typst package registry
//! 3. Extract and cache for future use
//!
//! # Cache Location
//!
//! Packages are cached at platform-specific locations:
//! - Linux: `~/.cache/typst/packages`
//! - macOS: `~/Library/Caches/typst/packages`
//! - Windows: `%LOCALAPPDATA%\typst\packages`
//!
//! # Thread Safety
//!
//! `PackageStorage` is thread-safe and can be shared across compilations.
//! Downloads are coordinated to prevent duplicate requests.
//!
//! # Usage
//!
//! The storage is accessed via file resolution in `file.rs`:
//!
//! ```ignore
//! if let Some(spec) = id.package() {
//!     let root = GLOBAL_PACKAGE_STORAGE.prepare_package(spec, &mut progress)?;
//!     // root now points to the downloaded package directory
//! }
//! ```

use std::sync::LazyLock;

use typst_kit::download::Downloader;
use typst_kit::package::PackageStorage;

/// Global shared package storage - one cache for all compilations.
///
/// Uses `LazyLock` for thread-safe, one-time initialization on first access.
///
/// # Configuration
///
/// - Cache path: Uses platform default (`~/.cache/typst/packages` on Linux)
/// - Package path: Uses platform default
/// - User-Agent: `tola/{version}` for package downloads
pub static GLOBAL_PACKAGE_STORAGE: LazyLock<PackageStorage> = LazyLock::new(|| {
    PackageStorage::new(
        None, // Use default cache path (~/.cache/typst/packages)
        None, // Use default package path
        // Create downloader with tola user-agent for attribution
        Downloader::new(format!("tola/{}", env!("CARGO_PKG_VERSION"))),
    )
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_initialized() {
        // Should not panic on access
        let _storage = &*GLOBAL_PACKAGE_STORAGE;
    }

    #[test]
    fn test_storage_is_shared() {
        let storage1 = &*GLOBAL_PACKAGE_STORAGE;
        let storage2 = &*GLOBAL_PACKAGE_STORAGE;
        // Should return the same static reference
        assert!(std::ptr::eq(storage1, storage2), "Storage should be shared");
    }
}
