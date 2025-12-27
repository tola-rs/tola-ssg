//! Virtual file system for `/_data/*.json` files.
//!
//! Intercepts file reads to `/_data/` paths and returns dynamically generated
//! JSON from the global site data store.

use std::path::{Path, PathBuf};

use super::store::GLOBAL_SITE_DATA;

/// Canonical virtual data directory path (used for dependency tracking).
pub const VIRTUAL_DATA_DIR: &str = "/_data";

/// Known virtual files and their generators.
/// Known virtual files and their generators.
type VirtualFileGenerator = fn() -> String;

const VIRTUAL_FILES: &[(&str, VirtualFileGenerator)] = &[
    ("pages.json", || GLOBAL_SITE_DATA.pages_to_json()),
    ("tags.json", || GLOBAL_SITE_DATA.tags_to_json()),
];

/// Check if a path refers to a virtual data file.
///
/// Virtual data files are in the `/_data/` directory and are generated
/// dynamically rather than read from disk.
///
/// # Arguments
///
/// * `path` - The path to check (can be absolute or relative)
///
/// # Returns
///
/// `true` if this is a virtual data path that should be intercepted.
pub fn is_virtual_data_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    // Handle both absolute paths and root-relative paths
    if path_str.contains("/_data/") {
        let suffix = path_str
            .rsplit("/_data/")
            .next()
            .unwrap_or("");

        return VIRTUAL_FILES.iter().any(|(name, _)| *name == suffix);
    }

    false
}

/// Read a virtual data file.
///
/// # Arguments
///
/// * `path` - The virtual file path (must pass `is_virtual_data_path` check)
///
/// # Returns
///
/// The JSON content of the virtual file, or an empty array/object if not found.
pub fn read_virtual_data(path: &Path) -> Option<Vec<u8>> {
    let path_str = path.to_string_lossy();

    let suffix = path_str
        .rsplit("/_data/")
        .next()?;

    for (name, generator) in VIRTUAL_FILES {
        if *name == suffix {
            return Some(generator().into_bytes());
        }
    }

    None
}

/// Get all virtual data file paths (for dependency graph queries).
///
/// These paths match what Typst templates use when calling `json("/_data/*.json")`.
pub fn virtual_data_paths() -> Vec<PathBuf> {
    VIRTUAL_FILES
        .iter()
        .map(|(name, _)| PathBuf::from(format!("{VIRTUAL_DATA_DIR}/{name}")))
        .collect()
}

/// Write all virtual data files to disk.
///
/// # Arguments
///
/// * `data_dir` - The full path to the data directory (e.g., `public/_data`)
pub fn write_to_disk(data_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;

    for (name, generator) in VIRTUAL_FILES {
        let path = data_dir.join(name);
        std::fs::write(&path, generator())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_virtual_data_path() {
        assert!(is_virtual_data_path(Path::new("/_data/pages.json")));
        assert!(is_virtual_data_path(Path::new("/_data/tags.json")));
        assert!(is_virtual_data_path(Path::new("/project/_data/pages.json")));
        assert!(is_virtual_data_path(Path::new("/some/path/_data/tags.json")));

        assert!(!is_virtual_data_path(Path::new("/_data/unknown.json")));
        assert!(!is_virtual_data_path(Path::new("/regular/file.json")));
        assert!(!is_virtual_data_path(Path::new("/_data/")));
        assert!(!is_virtual_data_path(Path::new("/data/pages.json")));
    }

    #[test]
    fn test_read_virtual_data_empty() {
        // Clear store to ensure empty state
        GLOBAL_SITE_DATA.clear();

        let pages = read_virtual_data(Path::new("/_data/pages.json"));
        assert!(pages.is_some());
        assert_eq!(String::from_utf8_lossy(&pages.unwrap()), "[]");

        let tags = read_virtual_data(Path::new("/_data/tags.json"));
        assert!(tags.is_some());
        assert_eq!(String::from_utf8_lossy(&tags.unwrap()), "{}");
    }

    #[test]
    fn test_read_unknown_virtual_file() {
        let result = read_virtual_data(Path::new("/_data/unknown.json"));
        assert!(result.is_none());
    }

    #[test]
    fn test_virtual_data_paths() {
        let paths = virtual_data_paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&PathBuf::from("/_data/pages.json")));
        assert!(paths.contains(&PathBuf::from("/_data/tags.json")));
    }

    #[test]
    fn test_write_to_disk() {
        use tempfile::TempDir;

        GLOBAL_SITE_DATA.clear();

        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("_data");
        write_to_disk(&data_dir).unwrap();

        assert!(data_dir.exists());
        assert!(data_dir.join("pages.json").exists());
        assert!(data_dir.join("tags.json").exists());

        let pages_content = std::fs::read_to_string(data_dir.join("pages.json")).unwrap();
        assert_eq!(pages_content, "[]");

        let tags_content = std::fs::read_to_string(data_dir.join("tags.json")).unwrap();
        assert_eq!(tags_content, "{}");
    }
}
