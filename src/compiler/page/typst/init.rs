//! Typst compilation support for tola-ssg.
//!
//! This module provides tola-specific Typst functionality:
//! - Virtual package system for `@tola/site:0.0.0` and `@tola/current:0.0.0`
//! - Virtual file system for nested asset path mapping
//! - Warmup function for font initialization

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use crate::package;

// =============================================================================
// Virtual File System
// =============================================================================

/// Nested asset mapping: (output_name, source_path)
///
/// Example: `("images", "assets/images")` maps `/images/photo.webp` to `assets/images/photo.webp`
pub type NestedMapping = (String, PathBuf);

/// Tola's virtual file system for:
/// - `@tola/site:0.0.0` and `@tola/current:0.0.0` packages
/// - Nested asset path mapping (e.g., `/images/xxx` -> `assets/images/xxx`)
pub struct TolaVirtualFS {
    root: PathBuf,
    nested_mappings: Vec<NestedMapping>,
}

impl TolaVirtualFS {
    /// Create a new VFS with nested asset mappings.
    pub fn new(root: PathBuf, nested_mappings: Vec<NestedMapping>) -> Self {
        Self { root, nested_mappings }
    }

    /// Create a VFS without nested mappings (for lightweight operations).
    pub fn without_mappings() -> Self {
        Self {
            root: PathBuf::new(),
            nested_mappings: Vec::new(),
        }
    }
}

impl typst_batch::VirtualFileSystem for TolaVirtualFS {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        let path_str = path.to_str()?;
        let trimmed = path_str.trim_start_matches('/');

        // Find first path segment
        let first_segment = trimmed.split('/').next()?;

        // Look for matching nested mapping
        for (output_name, source) in &self.nested_mappings {
            if output_name == first_segment {
                // "/images/photo.webp" -> "assets/images/photo.webp"
                let rest = trimmed.strip_prefix(first_segment).unwrap_or("");
                let rest = rest.trim_start_matches('/');
                let real_path = if rest.is_empty() {
                    source.clone()
                } else {
                    source.join(rest)
                };
                return std::fs::read(self.root.join(real_path)).ok();
            }
        }
        None
    }

    fn read_package(&self, pkg: &PackageId, path: &str) -> Option<Vec<u8>> {
        package::read_package(pkg, path)
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Initialize Typst compilation environment with nested asset mappings.
///
/// Call once at startup. This:
/// - Registers the virtual file system with nested asset path mappings
/// - Pre-warms fonts, library, package storage, and file cache
///
/// The `nested_mappings` parameter maps output names to source paths:
/// - `("images", "assets/images")` maps `/images/xxx` to `assets/images/xxx`
pub fn init_typst_with_mappings(
    font_dirs: &[&Path],
    root: PathBuf,
    nested_mappings: Vec<NestedMapping>,
) {
    typst_batch::set_virtual_fs(TolaVirtualFS::new(root, nested_mappings));
    typst_batch::warmup(font_dirs);
}

/// Initialize Typst compilation environment (legacy, no nested mappings).
///
/// Prefer `init_typst_with_mappings` for full nested asset support.
pub fn init_typst(font_dirs: &[&Path]) {
    typst_batch::set_virtual_fs(TolaVirtualFS::without_mappings());
    typst_batch::warmup(font_dirs);
}

/// Register only the virtual file system with nested mappings (no font warmup).
///
/// Use for lightweight operations like query/validate that don't need fonts.
pub fn init_vfs_with_mappings(root: PathBuf, nested_mappings: Vec<NestedMapping>) {
    typst_batch::set_virtual_fs(TolaVirtualFS::new(root, nested_mappings));
}

/// Register only the virtual file system (no font warmup, no nested mappings).
///
/// Use for lightweight operations like query/validate that don't need fonts.
pub fn init_vfs() {
    typst_batch::set_virtual_fs(TolaVirtualFS::without_mappings());
}

/// Build nested mappings from assets config.
pub fn build_nested_mappings(
    nested: &[crate::config::section::build::assets::NestedEntry],
) -> Vec<NestedMapping> {
    nested
        .iter()
        .map(|entry| (entry.output_name().to_string(), entry.source().to_path_buf()))
        .collect()
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

    #[test]
    fn test_init_with_mappings_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let mappings = vec![("images".to_string(), PathBuf::from("assets/images"))];
        init_typst_with_mappings(&[dir.path()], dir.path().to_path_buf(), mappings);
    }
}
