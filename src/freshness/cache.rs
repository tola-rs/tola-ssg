//! Global freshness cache for file content hashes.

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use super::ContentHash;

/// Global cache for file content hashes (thread-safe).
pub struct FreshnessCache {
    hashes: DashMap<PathBuf, ContentHash>,
}

impl FreshnessCache {
    pub fn new() -> Self {
        Self {
            hashes: DashMap::new(),
        }
    }

    pub fn get(&self, path: &Path) -> Option<ContentHash> {
        let canonical = path.canonicalize().ok()?;
        self.hashes.get(&canonical).map(|r| *r)
    }

    pub fn set(&self, path: &Path, hash: ContentHash) {
        if let Ok(canonical) = path.canonicalize() {
            self.hashes.insert(canonical, hash);
        }
    }

    #[allow(dead_code)]
    pub fn invalidate(&self, path: &Path) {
        if let Ok(canonical) = path.canonicalize() {
            self.hashes.remove(&canonical);
        }
    }

    pub fn clear(&self) {
        self.hashes.clear();
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.hashes.len()
    }
}

impl Default for FreshnessCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global freshness cache instance.
pub static FRESHNESS_CACHE: LazyLock<FreshnessCache> = LazyLock::new(FreshnessCache::new);

/// Get cached hash for a file.
#[inline]
pub fn get_cached_hash(path: &Path) -> Option<ContentHash> {
    FRESHNESS_CACHE.get(path)
}

/// Store hash in global cache.
#[inline]
pub fn set_cached_hash(path: &Path, hash: ContentHash) {
    FRESHNESS_CACHE.set(path, hash);
}

/// Clear the global freshness cache.
#[inline]
pub fn clear_cache() {
    FRESHNESS_CACHE.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_cache_get_set() {
        let cache = FreshnessCache::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "content").unwrap();

        let hash = ContentHash::new([1; 32]);
        cache.set(&path, hash);

        assert_eq!(cache.get(&path), Some(hash));
    }

    #[test]
    fn test_cache_invalidate() {
        let cache = FreshnessCache::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "content").unwrap();

        let hash = ContentHash::new([1; 32]);
        cache.set(&path, hash);
        cache.invalidate(&path);

        assert_eq!(cache.get(&path), None);
    }

    #[test]
    fn test_cache_clear() {
        let cache = FreshnessCache::new();
        let dir = TempDir::new().unwrap();

        let path1 = dir.path().join("a.txt");
        let path2 = dir.path().join("b.txt");
        fs::write(&path1, "a").unwrap();
        fs::write(&path2, "b").unwrap();

        cache.set(&path1, ContentHash::new([1; 32]));
        cache.set(&path2, ContentHash::new([2; 32]));
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert_eq!(cache.len(), 0);
    }
}
