//! Site directory structure creation.
//!
//! Creates the standard Tola site directory layout.

use anyhow::{Context, Result};
use std::{fs, path::Path};

/// Standard site directory structure.
const SITE_DIRS: &[&str] = &[
    "content",
    "assets/images",
    "assets/iconfonts",
    "assets/fonts",
    "assets/scripts",
    "assets/styles",
    "templates",
    "utils",
];

/// Create site directory structure at the given root.
///
/// Creates all standard directories. The root directory
/// is created if it doesn't exist.
pub fn create_structure(root: &Path) -> Result<()> {
    // Ensure root exists
    if !root.exists() {
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create root directory '{}'", root.display()))?;
    }

    // Create all subdirectories
    for dir in SITE_DIRS {
        let path = root.join(dir);
        fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create directory '{}'", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_structure() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("my_site");

        create_structure(&root).unwrap();

        assert!(root.join("content").is_dir());
        assert!(root.join("assets/images").is_dir());
        assert!(root.join("templates").is_dir());
    }

    #[test]
    fn test_create_structure_existing_root() {
        let temp = TempDir::new().unwrap();
        create_structure(temp.path()).unwrap();

        assert!(temp.path().join("content").is_dir());
    }
}
