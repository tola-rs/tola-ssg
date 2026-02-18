//! Cache index data structures.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// Index file name
pub const INDEX_FILE: &str = "index.json";

/// Information about a cached file entry
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheFileInfo {
    /// Filename for the cached vdom (without extension)
    pub filename: String,
    /// Source file path (relative to project root)
    pub source_path: String,
    /// Source file content hash (blake3 hex, for change detection)
    #[serde(default)]
    pub source_hash: String,
    /// Dependencies: relative path -> content hash at build time
    #[serde(default)]
    pub dependencies: FxHashMap<String, String>,
}

/// Index mapping URL paths to cache metadata
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CacheIndex {
    /// URL path -> cache file info
    pub entries: FxHashMap<String, CacheFileInfo>,
    /// Index creation time (Unix timestamp in seconds)
    #[serde(default)]
    pub created_at: u64,
}

impl CacheIndex {
    /// Create a new index with current timestamp.
    pub fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            created_at: current_timestamp(),
        }
    }

    /// Generate a safe filename from URL path.
    pub fn url_to_filename(url: &str) -> String {
        crate::utils::path::route::url_to_safe_filename(url)
    }
}

/// Get current Unix timestamp in seconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_to_filename() {
        assert_eq!(CacheIndex::url_to_filename("/"), "_");
        assert_eq!(CacheIndex::url_to_filename("/index"), "_index");
        assert_eq!(CacheIndex::url_to_filename("/blog"), "_blog");
        assert_eq!(CacheIndex::url_to_filename("/blog/post"), "_blog_post");
    }
}
