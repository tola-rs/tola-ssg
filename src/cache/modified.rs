//! Modified file detection via content hash comparison.

use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use super::CACHE_DIR;
use super::index::{CacheFileInfo, CacheIndex, INDEX_FILE};
use crate::core::UrlPath;

/// Result of modified files detection.
#[derive(Debug, Default)]
pub struct ModifiedFilesResult {
    /// Files that were modified since they were cached
    pub modified: Vec<PathBuf>,
    /// Source paths mapping (url → source path) for reuse
    pub source_paths: FxHashMap<UrlPath, PathBuf>,
}

/// Detect content files that were modified since they were cached.
pub fn get_modified_files(root: &Path) -> ModifiedFilesResult {
    let Some(index) = load_index(root) else {
        return ModifiedFilesResult::default();
    };

    let mut result = ModifiedFilesResult::default();

    for (url, info) in &index.entries {
        let Some(source_path) = resolve_source_path(root, info) else {
            continue;
        };

        result
            .source_paths
            .insert(UrlPath::from_page(url), source_path.clone());

        if is_file_modified(root, info) {
            result.modified.push(source_path);
        }
    }

    if !result.modified.is_empty() {
        crate::debug!("modified"; "{} files changed since cache", result.modified.len());
    }

    result
}

/// Load all source paths from cache index.
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

/// Load cache index from disk.
fn load_index(root: &Path) -> Option<CacheIndex> {
    let index_path = root.join(CACHE_DIR).join(INDEX_FILE);
    let json = fs::read_to_string(&index_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Compute file content hash (blake3 hex).
fn compute_hash(path: &Path) -> String {
    crate::freshness::compute_file_hash(path).to_hex()
}

/// Resolve source path from cache info (returns None if empty/invalid).
fn resolve_source_path(root: &Path, info: &CacheFileInfo) -> Option<PathBuf> {
    if info.source_path.is_empty() {
        return None;
    }
    Some(crate::utils::path::normalize_path(
        &root.join(&info.source_path),
    ))
}

/// Check if a cached file has been modified (source or deps changed).
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
