//! Modified file detection via content hash comparison.

use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};

use super::CACHE_DIR;
use super::index::{CacheFileInfo, CacheIndex, INDEX_FILE};
use crate::core::{ContentKind, UrlPath};

/// Result of modified files detection
#[derive(Debug, Default)]
pub struct ModifiedFilesResult {
    /// New content files created while server was offline
    pub created: Vec<PathBuf>,
    /// Files removed while server was offline
    pub removed: Vec<RemovedFile>,
    /// Files that were modified since they were cached
    pub modified: Vec<PathBuf>,
    /// Source paths mapping (url -> source path) for reuse
    pub source_paths: FxHashMap<UrlPath, PathBuf>,
    /// Cached URL mapping by source path (source -> url)
    pub cached_urls_by_source: FxHashMap<PathBuf, UrlPath>,
}

/// Removed file information from cache index
#[derive(Debug, Clone)]
pub struct RemovedFile {
    pub source_path: PathBuf,
    pub url_path: UrlPath,
}

impl RemovedFile {
    /// Build a removed-file record from a cache index entry.
    ///
    /// Returns `None` when:
    /// - source path is invalid/empty
    /// - source file still exists (not removed)
    pub fn new(root: &Path, url: &str, info: &CacheFileInfo) -> Option<Self> {
        let source_path = resolve_source_path(root, info)?;
        if source_path.exists() {
            return None;
        }

        Some(Self {
            source_path,
            url_path: UrlPath::from_page(url),
        })
    }
}

/// Detect offline content changes since cache was written.
///
/// Returns three categories:
/// - `created`: exists now, not in cache index
/// - `removed`: existed in cache index, now missing
/// - `modified`: existed in cache index and hash changed
pub fn get_modified_files(root: &Path, content_dir: &Path) -> ModifiedFilesResult {
    // Ensure hash comparisons reflect current on-disk content.
    crate::freshness::clear_cache();

    let Some(index) = load_index(root) else {
        return ModifiedFilesResult::default();
    };

    let mut result = ModifiedFilesResult::default();
    let current_content = collect_content_files(content_dir);
    let mut modified_set = FxHashSet::default();

    for (url, info) in &index.entries {
        let Some(source_path) = resolve_source_path(root, info) else {
            continue;
        };

        let url_path = UrlPath::from_page(url);

        result
            .source_paths
            .insert(url_path.clone(), source_path.clone());
        result
            .cached_urls_by_source
            .insert(source_path.clone(), url_path.clone());

        if let Some(removed) = RemovedFile::new(root, url, info) {
            result.removed.push(removed);
            continue;
        }

        if is_file_modified(root, info) {
            modified_set.insert(source_path);
        }
    }

    result.created = current_content
        .into_iter()
        .filter(|path| !result.cached_urls_by_source.contains_key(path))
        .collect();
    result.modified = modified_set.into_iter().collect();

    result.created.sort();
    result.modified.sort();
    result
        .removed
        .sort_by(|a, b| a.source_path.cmp(&b.source_path));

    if !result.created.is_empty() || !result.removed.is_empty() || !result.modified.is_empty() {
        crate::debug!(
            "modified";
            "offline changes: created={}, removed={}, modified={}",
            result.created.len(),
            result.removed.len(),
            result.modified.len()
        );
    }

    result
}

/// Load all source paths from cache index
pub fn get_source_paths(root: &Path) -> FxHashMap<UrlPath, PathBuf> {
    let Some(index) = load_index(root) else {
        return FxHashMap::default();
    };

    let paths: FxHashMap<_, _> = index
        .entries
        .iter()
        .filter_map(|(url, info)| {
            resolve_source_path(root, info).map(|p| (UrlPath::from_page(url), p))
        })
        .collect();

    crate::debug!("modified"; "loaded {} source paths", paths.len());
    paths
}

/// Load cache index from disk
fn load_index(root: &Path) -> Option<CacheIndex> {
    let index_path = root.join(CACHE_DIR).join(INDEX_FILE);
    let json = fs::read_to_string(&index_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Collect current content files from the configured content directory.
fn collect_content_files(content_dir: &Path) -> FxHashSet<PathBuf> {
    if !content_dir.exists() {
        return FxHashSet::default();
    }

    crate::compiler::collect_all_files(content_dir)
        .into_iter()
        .filter(|path| ContentKind::from_path(path).is_some())
        .map(|path| crate::utils::path::normalize_path(&path))
        .collect()
}

/// Compute file content hash (blake3 hex)
fn compute_hash(path: &Path) -> String {
    crate::freshness::compute_file_hash(path).to_hex()
}

/// Resolve source path from cache info (returns None if empty/invalid)
fn resolve_source_path(root: &Path, info: &CacheFileInfo) -> Option<PathBuf> {
    if info.source_path.is_empty() {
        return None;
    }
    Some(crate::utils::path::normalize_path(
        &root.join(&info.source_path),
    ))
}

/// Check if a cached file has been modified (source or deps changed)
fn is_file_modified(root: &Path, info: &CacheFileInfo) -> bool {
    // Check source hash
    let source_path = root.join(&info.source_path);
    if compute_hash(&source_path) != info.source_hash {
        crate::debug!("modified"; "{} (source changed)", info.source_path);
        return true;
    }

    // Check dependency hashes
    for (dep_rel, cached_hash) in &info.dependencies {
        let dep_path = root.join(dep_rel);
        if compute_hash(&dep_path) != *cached_hash {
            crate::debug!("modified"; "{} (dep {} changed)", info.source_path, dep_rel);
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::cache::index::{CacheFileInfo, CacheIndex};

    fn write_index(root: &Path, index: &CacheIndex) {
        let cache_dir = root.join(CACHE_DIR);
        fs::create_dir_all(&cache_dir).unwrap();
        let index_path = cache_dir.join(INDEX_FILE);
        fs::write(index_path, serde_json::to_string_pretty(index).unwrap()).unwrap();
    }

    fn make_entry(root: &Path, rel_source: &str, filename: &str) -> CacheFileInfo {
        let source_abs = root.join(rel_source);
        CacheFileInfo {
            filename: filename.to_string(),
            source_path: rel_source.to_string(),
            source_hash: compute_hash(&source_abs),
            dependencies: FxHashMap::default(),
        }
    }

    #[test]
    fn test_detect_created_removed_modified_files() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();

        let unchanged = content.join("unchanged.typ");
        let modified = content.join("modified.typ");
        let created = content.join("created.typ");
        let removed_rel = "content/removed.typ";

        fs::write(&unchanged, "= unchanged").unwrap();
        fs::write(&modified, "= old").unwrap();

        let mut index = CacheIndex::new();
        index.entries.insert(
            "/unchanged/".to_string(),
            make_entry(root, "content/unchanged.typ", "unchanged"),
        );
        index.entries.insert(
            "/modified/".to_string(),
            make_entry(root, "content/modified.typ", "modified"),
        );
        index.entries.insert(
            "/removed/".to_string(),
            CacheFileInfo {
                filename: "removed".to_string(),
                source_path: removed_rel.to_string(),
                source_hash: "old-hash".to_string(),
                dependencies: FxHashMap::default(),
            },
        );

        write_index(root, &index);

        fs::write(&modified, "= new").unwrap();
        fs::write(&created, "= created").unwrap();

        let result = get_modified_files(root, &content);

        assert_eq!(result.created.len(), 1);
        assert!(
            result
                .created
                .contains(&crate::utils::path::normalize_path(&created))
        );

        assert_eq!(result.modified.len(), 1);
        assert!(
            result
                .modified
                .contains(&crate::utils::path::normalize_path(&modified))
        );

        assert_eq!(result.removed.len(), 1);
        assert_eq!(
            result.removed[0].source_path,
            crate::utils::path::normalize_path(&root.join(removed_rel))
        );
        assert_eq!(result.removed[0].url_path, UrlPath::from_page("/removed/"));
    }
}
