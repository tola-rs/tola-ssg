//! Dependency tracking for incremental builds.
//!
//! Three-layer architecture:
//! - `DependencyGraph`: Pure data structure with forward/reverse mappings
//! - `global`: Thread-safe singleton for accessing the graph
//! - `parallel`: Lock-free accumulation during par_iter compilation

use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::utils::path::normalize_path;

type PathSet = FxHashSet<PathBuf>;
type PathSetMap = FxHashMap<PathBuf, PathSet>;
type DepEntry = (PathBuf, Vec<PathBuf>);

// =============================================================================
// Layer 1: Data Structure
// =============================================================================

/// Bidirectional dependency graph for incremental builds.
///
/// Maintains both forward (content → deps) and reverse (dep → contents) mappings
/// for efficient lookups in either direction.
///
/// # Invariants
/// - Forward and reverse mappings are always consistent
/// - Paths are normalized for reliable matching
/// - Self-references are excluded
#[derive(Debug, Default)]
pub struct DependencyGraph {
    /// Forward: content file → its dependencies (templates, utils, packages)
    forward: PathSetMap,
    /// Reverse: dependency → content files that use it
    reverse: PathSetMap,
}

impl DependencyGraph {
    /// Create an empty dependency graph.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record dependencies for a content file.
    ///
    /// Replaces any existing dependencies for this file.
    /// Paths are normalized for consistent matching.
    pub fn record(&mut self, content_file: &Path, accessed_files: &[PathBuf]) {
        let content = normalize_path(content_file);

        // Remove old mappings first (maintains invariant)
        self.remove_content(&content);

        // Build new dependency set (exclude self-reference, normalize paths)
        let deps: PathSet = accessed_files
            .iter()
            .filter(|p| p.as_path() != content.as_path())
            .map(|p| normalize_path(p))
            .collect();

        // Update reverse mapping
        for dep in &deps {
            self.reverse
                .entry(dep.clone())
                .or_default()
                .insert(content.clone());
        }

        // Store forward mapping
        self.forward.insert(content, deps);
    }

    /// Get content files that depend on the given file.
    #[inline]
    pub fn used_by(&self, file: &Path) -> Option<&PathSet> {
        self.reverse.get(file)
    }

    /// Get dependencies of a content file.
    #[inline]
    pub fn uses(&self, content_file: &Path) -> Option<&PathSet> {
        self.forward.get(content_file)
    }

    /// Clear all mappings.
    #[inline]
    pub fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
    }

    /// Number of tracked dependencies (for debugging).
    #[inline]
    pub fn reverse_count(&self) -> usize {
        self.reverse.len()
    }

    // -------------------------------------------------------------------------
    // Private
    // -------------------------------------------------------------------------

    /// Remove a content file and clean up its reverse mappings.
    fn remove_content(&mut self, content: &Path) {
        let Some(old_deps) = self.forward.remove(content) else {
            return;
        };

        for dep in old_deps {
            if let Some(dependents) = self.reverse.get_mut(&dep) {
                dependents.remove(content);
                if dependents.is_empty() {
                    self.reverse.remove(&dep);
                }
            }
        }
    }
}

// =============================================================================
// Layer 2: Global State (Thread-Safe Singleton)
// =============================================================================

/// Global dependency graph access.
///
/// Isolates mutable global state behind a clean interface.
pub mod global {
    use super::*;

    /// Global dependency graph instance.
    static GRAPH: LazyLock<RwLock<DependencyGraph>> =
        LazyLock::new(|| RwLock::new(DependencyGraph::new()));

    /// Access the graph with a read lock.
    ///
    /// Use this when you need multiple queries in a loop to avoid
    /// repeated lock acquisition.
    pub fn with_read<F, R>(f: F) -> R
    where
        F: FnOnce(&DependencyGraph) -> R,
    {
        f(&GRAPH.read())
    }

    /// Get content files that use the given file.
    ///
    /// Returns empty vec if none found.
    pub fn used_by(file: &Path) -> Vec<PathBuf> {
        let normalized = normalize_path(file);
        GRAPH
            .read()
            .used_by(&normalized)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get files that a content file uses.
    ///
    /// Returns None if no dependencies recorded.
    #[allow(dead_code)]
    pub fn uses(content_file: &Path) -> Option<Vec<PathBuf>> {
        let normalized = normalize_path(content_file);
        GRAPH
            .read()
            .uses(&normalized)
            .map(|set| set.iter().cloned().collect())
    }

    /// Record dependencies for a content file.
    ///
    /// Used by cache restoration to populate the graph.
    pub fn record(content_file: &Path, accessed_files: &[PathBuf]) {
        GRAPH.write().record(content_file, accessed_files);
    }

    /// Clear the global dependency graph.
    ///
    /// Called during full rebuild to reset tracking state.
    pub fn clear() {
        GRAPH.write().clear();
    }

    /// Merge dependency entries into the global graph.
    ///
    /// Used by [`parallel::flush_to_global`] after collecting from all threads.
    pub(super) fn merge(entries: impl Iterator<Item = DepEntry>) -> usize {
        let mut graph = GRAPH.write();
        for (content, deps) in entries {
            graph.record(&content, &deps);
        }
        graph.reverse_count()
    }
}

// =============================================================================
// Layer 3: Parallel Collection (Lock-Free Accumulation)
// =============================================================================

/// Lock-free dependency collection for parallel compilation.
///
/// During `par_iter()`, each thread accumulates dependencies locally.
/// After completion, `flush_to_global()` merges all data with a single lock.
pub mod parallel {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// Thread-local accumulator (no locks needed).
        static LOCAL: RefCell<Vec<DepEntry>> = const { RefCell::new(Vec::new()) };
    }

    /// Record dependencies to thread-local storage.
    ///
    /// Call this during parallel compilation. Lock-free operation.
    pub fn record_local(content_file: &Path, accessed_files: Vec<PathBuf>) {
        LOCAL.with(|deps| {
            deps.borrow_mut()
                .push((content_file.to_path_buf(), accessed_files));
        });
    }

    /// Flush all thread-local dependencies to the global graph.
    ///
    /// Call once after `par_iter()` completes. Collects from all rayon workers
    /// and the main thread, then merges with a single write lock.
    pub fn flush_to_global() {
        // Collect from all rayon worker threads
        let rayon_deps: Vec<Vec<DepEntry>> =
            rayon::broadcast(|_| LOCAL.with(|deps| std::mem::take(&mut *deps.borrow_mut())));

        // Collect from main thread (may not be a rayon worker)
        let main_deps: Vec<DepEntry> = LOCAL.with(|deps| std::mem::take(&mut *deps.borrow_mut()));

        // Stats for debugging
        let rayon_count: usize = rayon_deps.iter().map(|v| v.len()).sum();
        let main_count = main_deps.len();

        // Merge all into global graph (single lock acquisition)
        let all_entries = rayon_deps.into_iter().flatten().chain(main_deps);
        let reverse_count = global::merge(all_entries);

        crate::debug!("dep"; "flushed {} rayon + {} main deps, reverse map has {} entries",
            rayon_count, main_count, reverse_count);
    }
}

// =============================================================================
// Public API
// =============================================================================

pub use global::{clear as clear_graph, used_by as get_dependents};
pub use parallel::{
    flush_to_global as flush_thread_local_deps, record_local as record_dependencies_local,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    mod dependency_graph {
        use super::*;

        #[test]
        fn new_graph_is_empty() {
            let graph = DependencyGraph::new();
            assert!(graph.used_by(&path("/any.typ")).is_none());
        }

        #[test]
        fn basic_recording() {
            let mut graph = DependencyGraph::new();

            let content = path("/project/content/index.typ");
            let template = path("/project/templates/base.typ");

            graph.record(&content, std::slice::from_ref(&template));

            let users = graph.used_by(&template).unwrap();
            assert!(users.contains(&content));
        }

        #[test]
        fn self_reference_excluded() {
            let mut graph = DependencyGraph::new();

            let content = path("/project/content/index.typ");
            let template = path("/project/templates/base.typ");

            // Include content file itself (should be filtered out)
            graph.record(&content, &[content.clone(), template.clone()]);

            assert!(graph.used_by(&content).is_none());
            assert!(graph.used_by(&template).unwrap().contains(&content));
        }

        #[test]
        fn update_replaces_old_dependencies() {
            let mut graph = DependencyGraph::new();

            let content = path("/project/content/index.typ");
            let old_template = path("/project/templates/old.typ");
            let new_template = path("/project/templates/new.typ");

            graph.record(&content, std::slice::from_ref(&old_template));
            assert!(graph.used_by(&old_template).is_some());

            graph.record(&content, std::slice::from_ref(&new_template));

            // Old dependency cleaned up
            assert!(graph.used_by(&old_template).is_none());
            // New dependency exists
            assert!(graph.used_by(&new_template).unwrap().contains(&content));
        }

        #[test]
        fn multiple_contents_share_dependency() {
            let mut graph = DependencyGraph::new();

            let content1 = path("/project/content/a.typ");
            let content2 = path("/project/content/b.typ");
            let shared = path("/project/templates/shared.typ");

            graph.record(&content1, std::slice::from_ref(&shared));
            graph.record(&content2, std::slice::from_ref(&shared));

            let users = graph.used_by(&shared).unwrap();
            assert_eq!(users.len(), 2);
            assert!(users.contains(&content1));
            assert!(users.contains(&content2));
        }

        #[test]
        fn clear_removes_all() {
            let mut graph = DependencyGraph::new();

            let template = path("/templates/base.typ");
            graph.record(&path("/a.typ"), std::slice::from_ref(&template));
            graph.record(&path("/c.typ"), std::slice::from_ref(&path("/d.typ")));

            graph.clear();

            assert!(graph.used_by(&template).is_none());
        }

        #[test]
        fn multiple_dependencies_per_file() {
            let mut graph = DependencyGraph::new();

            let content = path("/content/index.typ");
            let deps = vec![
                path("/templates/base.typ"),
                path("/utils/helper.typ"),
                path("/utils/date.typ"),
            ];

            graph.record(&content, &deps);

            for dep in &deps {
                assert!(graph.used_by(dep).unwrap().contains(&content));
            }
        }

        #[test]
        fn empty_dependencies() {
            let mut graph = DependencyGraph::new();

            let content = path("/content/index.typ");
            graph.record(&content, &[]);

            assert!(graph.used_by(&content).is_none());
        }

        #[test]
        fn nonexistent_returns_none() {
            let graph = DependencyGraph::new();
            assert!(graph.used_by(&path("/nonexistent.typ")).is_none());
        }
    }
}
