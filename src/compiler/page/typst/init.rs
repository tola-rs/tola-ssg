//! Typst compilation support for tola-ssg.
//!
//! This module provides tola-specific Typst functionality:
//! - Virtual package system for `@tola/site:0.0.0` and `@tola/current:0.0.0`
//! - Warmup function for font initialization

use std::path::Path;

use typst_batch::prelude::*;

use crate::package;

// =============================================================================
// Virtual File System
// =============================================================================

/// Tola's virtual file system for `@tola/site:0.0.0` and `@tola/current:0.0.0` packages
pub struct TolaVirtualFS;

impl typst_batch::VirtualFileSystem for TolaVirtualFS {
    fn read(&self, _path: &Path) -> Option<Vec<u8>> {
        // No virtual files - use @tola/site package instead
        None
    }

    fn read_package(&self, pkg: &PackageId, path: &str) -> Option<Vec<u8>> {
        package::read_package(pkg, path)
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Initialize Typst compilation environment
///
/// Call once at startup. This:
/// - Registers the virtual file system for `/_data/*.json` files
/// - Pre-warms fonts, library, package storage, and file cache
pub fn init_typst(font_dirs: &[&Path]) {
    typst_batch::set_virtual_fs(TolaVirtualFS);
    typst_batch::warmup(font_dirs);
}

/// Register only the virtual file system (no font warmup)
///
/// Use for lightweight operations like query/validate that don't need fonts
pub fn init_vfs() {
    typst_batch::set_virtual_fs(TolaVirtualFS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_does_not_panic() {
        let dir = TempDir::new().unwrap();
        init_typst(&[dir.path()]);
    }
}
