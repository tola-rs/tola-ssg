//! VDOM cache persistence.

use std::fs;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::compiler::dependency::DependencyGraph;
use crate::compiler::family::{CacheEntry, SharedCache};
use crate::core::UrlPath;
use tola_vdom::CacheKey;
use tola_vdom::serialize::{from_bytes_to_indexed, to_bytes};

use super::CACHE_DIR;
use super::index::{CacheFileInfo, CacheIndex, INDEX_FILE};

/// Entry ready to be persisted to disk
struct PersistEntry {
    url: String,
    filename: String,
    bytes: Vec<u8>,
    info: CacheFileInfo,
}

/// Persist the VDOM cache to disk
pub fn persist_cache(
    cache: &SharedCache,
    source_paths: &FxHashMap<UrlPath, PathBuf>,
    root: &Path,
) -> std::io::Result<usize> {
    let cache_dir = root.join(CACHE_DIR);
    fs::create_dir_all(&cache_dir)?;

    // Collect data (read locks held briefly)
    let entries = collect_entries_to_persist(cache, source_paths, root);

    // Write to disk (no locks held)
    let mut index = CacheIndex::new();
    let mut saved = 0;

    for entry in entries {
        if write_entry(&cache_dir, &entry).is_ok() {
            index.entries.insert(entry.url, entry.info);
            saved += 1;
        }
    }

    // Write index to disk
    write_index(&cache_dir, &index)?;

    crate::debug!("persist"; "saved {} entries to {}", saved, cache_dir.display());
    Ok(saved)
}

/// Restore the VDOM cache from disk
pub fn restore_cache(cache: &SharedCache, root: &Path) -> std::io::Result<usize> {
    let Some(index) = load_cache_index(root)? else {
        return Ok(0);
    };

    let cache_dir = root.join(CACHE_DIR);

    // Read and parse files (no locks)
    let entries = collect_entries_to_restore(&index, &cache_dir);
    let restored = entries.len();

    // Insert into cache (write lock held briefly)
    cache.with_write(|c| {
        c.extend(
            entries
                .into_iter()
                .map(|(url, doc)| (CacheKey::new(&url), CacheEntry::with_default_version(doc))),
        );
    });

    crate::debug!("persist"; "restored {} entries from {}", restored, cache_dir.display());
    Ok(restored)
}

/// Restore the dependency graph from cached index
pub fn restore_dependency_graph(root: &Path) -> std::io::Result<usize> {
    let Some(index) = load_cache_index(root)? else {
        return Ok(0);
    };

    let dep_entries = collect_dependency_entries(&index, root);
    if dep_entries.is_empty() {
        return Ok(0);
    }

    let count = dep_entries.len();
    for (source, deps) in dep_entries {
        crate::compiler::dependency::global::record(&source, &deps);
    }

    crate::debug!("persist"; "restored {} dependency entries", count);
    Ok(count)
}

/// Check if valid VDOM cache exists
pub fn has_cache(root: &Path) -> bool {
    root.join(CACHE_DIR).join(INDEX_FILE).exists()
}

/// Clear the cache directory
pub fn clear_cache_dir(root: &Path) -> std::io::Result<()> {
    let cache_dir = root.join(CACHE_DIR);
    if cache_dir.exists() {
        fs::remove_dir_all(&cache_dir)?;
    }
    Ok(())
}

/// Collect cache entries ready for persistence
///
/// Acquires read locks on cache and dependency graph, extracts all data needed,
/// then releases locks before any IO happens
fn collect_entries_to_persist(
    cache: &SharedCache,
    source_paths: &FxHashMap<UrlPath, PathBuf>,
    root: &Path,
) -> Vec<PersistEntry> {
    let mut entries = Vec::new();

    cache.with_read(|c| {
        crate::compiler::dependency::global::with_read(|dep_graph| {
            for (key, cache_entry) in c.iter() {
                if let Some(entry) =
                    build_persist_entry(key.as_str(), cache_entry, source_paths, dep_graph, root)
                {
                    entries.push(entry);
                }
            }
        });
    });

    entries
}

/// Build a single persist entry from cache data
fn build_persist_entry(
    url: &str,
    cache_entry: &CacheEntry,
    source_paths: &FxHashMap<UrlPath, PathBuf>,
    dep_graph: &DependencyGraph,
    root: &Path,
) -> Option<PersistEntry> {
    // Serialize document
    let bytes = match to_bytes(&cache_entry.doc) {
        Ok(b) => b,
        Err(e) => {
            crate::debug!("persist"; "failed to serialize {}: {}", url, e);
            return None;
        }
    };

    let filename = CacheIndex::url_to_filename(url);

    // Build source info
    let (source_path, source_hash) = source_paths
        .get(url)
        .map(|p| {
            let rel = p.strip_prefix(root).unwrap_or(p);
            (rel.display().to_string(), compute_hash(p))
        })
        .unwrap_or_else(|| {
            crate::debug!("persist"; "no source path for {}", url);
            (String::new(), String::new())
        });

    // Build dependency info
    let dependencies = source_paths
        .get(url)
        .map(|p| crate::utils::path::normalize_path(p))
        .and_then(|normalized| dep_graph.uses(&normalized))
        .map(|deps| build_dependency_hashes(deps, root))
        .unwrap_or_default();

    Some(PersistEntry {
        url: url.to_string(),
        filename: filename.clone(),
        bytes,
        info: CacheFileInfo {
            filename,
            source_path,
            source_hash,
            dependencies,
        },
    })
}

/// Build dependency hash map from path set
fn build_dependency_hashes(
    deps: &rustc_hash::FxHashSet<PathBuf>,
    root: &Path,
) -> FxHashMap<String, String> {
    deps.iter()
        .map(|dep| {
            let rel = dep.strip_prefix(root).unwrap_or(dep);
            (rel.display().to_string(), compute_hash(dep))
        })
        .collect()
}

/// Collect entries to restore from disk
fn collect_entries_to_restore(
    index: &CacheIndex,
    cache_dir: &Path,
) -> Vec<(String, crate::compiler::family::IndexedDocument)> {
    index
        .entries
        .iter()
        .filter_map(|(url, info)| {
            read_entry(cache_dir, &info.filename)
                .ok()
                .map(|doc| (url.clone(), doc))
        })
        .collect()
}

/// Extract dependency entries from index
fn collect_dependency_entries(index: &CacheIndex, root: &Path) -> Vec<(PathBuf, Vec<PathBuf>)> {
    index
        .entries
        .values()
        .filter(|info| !info.source_path.is_empty() && !info.dependencies.is_empty())
        .map(|info| {
            let source = root.join(&info.source_path);
            let deps: Vec<PathBuf> = info.dependencies.keys().map(|rel| root.join(rel)).collect();
            (source, deps)
        })
        .collect()
}

/// Write a single cache entry to disk
fn write_entry(cache_dir: &Path, entry: &PersistEntry) -> std::io::Result<()> {
    let path = cache_dir.join(format!("{}.vdom", &entry.filename));
    fs::write(&path, &entry.bytes).map_err(|e| {
        crate::debug!("persist"; "failed to write {}: {}", path.display(), e);
        e
    })
}

/// Write index file to disk
fn write_index(cache_dir: &Path, index: &CacheIndex) -> std::io::Result<()> {
    let path = cache_dir.join(INDEX_FILE);
    let json = serde_json::to_string_pretty(index)?;
    fs::write(&path, json)
}

/// Read and deserialize a single cache entry
fn read_entry(
    cache_dir: &Path,
    filename: &str,
) -> Result<crate::compiler::family::IndexedDocument, String> {
    let path = cache_dir.join(format!("{}.vdom", filename));

    let bytes = fs::read(&path).map_err(|e| {
        crate::debug!("persist"; "failed to read {}: {}", path.display(), e);
        e.to_string()
    })?;

    from_bytes_to_indexed(&bytes).map_err(|e| {
        crate::debug!("persist"; "failed to deserialize {}: {}", path.display(), e);
        e.to_string()
    })
}

/// Load cache index from disk
fn load_cache_index(root: &Path) -> std::io::Result<Option<CacheIndex>> {
    let path = root.join(CACHE_DIR).join(INDEX_FILE);

    if !path.exists() {
        return Ok(None);
    }

    let json = fs::read_to_string(&path)?;
    let index: CacheIndex = serde_json::from_str(&json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    Ok(Some(index))
}

/// Compute file content hash (blake3 hex)
fn compute_hash(path: &Path) -> String {
    crate::freshness::compute_file_hash(path).to_hex()
}
