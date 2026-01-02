//! Pre-initialization validation.
//!
//! Validates target directory state before site creation.

use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

/// Initialization mode determines validation rules.
#[derive(Debug, Clone, Copy)]
pub enum InitMode {
    /// `tola init` - initialize in current directory (must be empty)
    CurrentDir,
    /// `tola init <name>` - create new subdirectory (must not exist)
    NewDir,
}

/// Validate target directory for initialization.
///
/// # Rules
/// - `CurrentDir`: directory must be empty (or not exist)
/// - `NewDir`: directory must not exist
pub fn validate_target(root: &Path, mode: InitMode) -> Result<()> {
    match mode {
        InitMode::CurrentDir => {
            if !is_empty(root)? {
                bail!(
                    "Current directory is not empty.\n\
                     Use `tola init <name>` to create in a new subdirectory."
                );
            }
        }
        InitMode::NewDir => {
            if root.exists() {
                bail!(
                    "Directory '{}' already exists.\n\
                     Choose a different name or remove the existing directory.",
                    root.display()
                );
            }
        }
    }
    Ok(())
}

/// Check if directory is empty or doesn't exist.
fn is_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(true);
    }
    let is_empty = fs::read_dir(path)
        .with_context(|| format!("Failed to read directory '{}'", path.display()))?
        .next()
        .is_none();
    Ok(is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_empty_dir_current_mode() {
        let temp = TempDir::new().unwrap();
        assert!(validate_target(temp.path(), InitMode::CurrentDir).is_ok());
    }

    #[test]
    fn test_non_empty_dir_current_mode() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file.txt"), "content").unwrap();
        assert!(validate_target(temp.path(), InitMode::CurrentDir).is_err());
    }

    #[test]
    fn test_existing_dir_new_mode() {
        let temp = TempDir::new().unwrap();
        assert!(validate_target(temp.path(), InitMode::NewDir).is_err());
    }

    #[test]
    fn test_non_existing_dir_new_mode() {
        let temp = TempDir::new().unwrap();
        let new_path = temp.path().join("new_site");
        assert!(validate_target(&new_path, InitMode::NewDir).is_ok());
    }
}
