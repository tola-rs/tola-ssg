//! Asset version management for cache busting.
//!
//! Uses content hash to generate version strings for assets.
//! When asset content changes, version changes, triggering browser re-fetch.

use std::path::{Path, PathBuf};

use dashmap::DashMap;
use std::sync::LazyLock;

/// Asset path → version hash mapping.
///
/// Thread-safe global storage for asset versions.
pub static ASSET_VERSIONS: LazyLock<DashMap<PathBuf, String>> = LazyLock::new(DashMap::new);

/// Compute version hash from file content (first 8 hex chars).
pub fn compute_version(path: &Path) -> String {
    let content = std::fs::read(path).unwrap_or_default();
    let hash = crate::utils::hash::compute(&content);
    format!("{:016x}", hash).chars().take(8).collect()
}

/// Get versioned URL for an asset.
///
/// Returns `base_url?v=abc12345` format.
pub fn versioned_url(base_url: &str, path: &Path) -> String {
    let version = ASSET_VERSIONS
        .get(path)
        .map(|v| v.clone())
        .unwrap_or_else(|| {
            let v = compute_version(path);
            ASSET_VERSIONS.insert(path.to_path_buf(), v.clone());
            v
        });
    format!("{}?v={}", base_url, version)
}

/// Update asset version and return whether it changed.
pub fn update_version(path: &Path) -> bool {
    let new_version = compute_version(path);
    let changed = ASSET_VERSIONS
        .get(path)
        .map(|old| *old != new_version)
        .unwrap_or(true);

    if changed {
        ASSET_VERSIONS.insert(path.to_path_buf(), new_version);
    }
    changed
}

/// Clear all cached versions.
pub fn clear() {
    ASSET_VERSIONS.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_compute_version() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.css");
        fs::write(&file, "body { color: red; }").unwrap();

        let v1 = compute_version(&file);
        assert_eq!(v1.len(), 8);

        // Same content = same version
        let v2 = compute_version(&file);
        assert_eq!(v1, v2);

        // Different content = different version
        fs::write(&file, "body { color: blue; }").unwrap();
        let v3 = compute_version(&file);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_versioned_url() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("style.css");
        fs::write(&file, "body {}").unwrap();

        let url = versioned_url("/style.css", &file);
        assert!(url.starts_with("/style.css?v="));
        assert_eq!(url.len(), "/style.css?v=".len() + 8);
    }

    #[test]
    fn test_update_version() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("app.js");
        fs::write(&file, "console.log(1)").unwrap();

        // First update = changed
        assert!(update_version(&file));

        // Same content = not changed
        assert!(!update_version(&file));

        // Modify content = changed
        fs::write(&file, "console.log(2)").unwrap();
        assert!(update_version(&file));
    }
}
