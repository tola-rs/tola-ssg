//! Dependency tracking for incremental builds.
//!
//! Tracks which content files depend on which templates/utils files,
//! enabling precise rebuilds when shared files change.
//!
//! # Architecture
//!
//! ```text
//! DependencyGraph
//! ├── forward: content_file → {template1.typ, utils/a.typ, ...}
//! └── reverse: template.typ → {content1.typ, content2.typ, ...}
//!
//! On template change:
//! 1. Lookup reverse[template.typ] → affected content files
//! 2. Rebuild only those content files
//! ```

use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// =============================================================================
// Global Dependency Graph
// =============================================================================

/// Global dependency graph for incremental builds.
///
/// Thread-safe and persists across watch mode rebuilds.
pub static DEPENDENCY_GRAPH: LazyLock<RwLock<DependencyGraph>> =
    LazyLock::new(|| RwLock::new(DependencyGraph::new()));

// =============================================================================
// DependencyGraph
// =============================================================================

/// Tracks file dependencies for precise incremental rebuilds.
///
/// When a template or utility file changes, we can quickly find all content
/// files that depend on it and rebuild only those.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    /// Forward mapping: content file → set of dependencies (templates, utils, packages)
    forward: FxHashMap<PathBuf, FxHashSet<PathBuf>>,
    /// Reverse mapping: dependency → set of content files that use it
    reverse: FxHashMap<PathBuf, FxHashSet<PathBuf>>,
}

impl DependencyGraph {
    /// Create a new empty dependency graph.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record dependencies for a content file after compilation.
    ///
    /// Call this after compiling a `.typ` file with the list of accessed files.
    pub fn record_dependencies(&mut self, content_file: &Path, accessed_files: &[PathBuf]) {
        // Remove old mappings first
        self.remove_forward_entry(content_file);

        // Build new dependency set (excluding the content file itself)
        let deps: FxHashSet<PathBuf> = accessed_files
            .iter()
            .filter(|p| p.as_path() != content_file)
            .cloned()
            .collect();

        let content_file = content_file.to_path_buf();

        // Update reverse mapping
        for dep in &deps {
            self.reverse
                .entry(dep.clone())
                .or_default()
                .insert(content_file.clone());
        }

        // Store forward mapping
        self.forward.insert(content_file, deps);
    }

    /// Get all content files that depend on the given file.
    #[inline]
    pub fn get_dependents(&self, dependency: &Path) -> Option<&FxHashSet<PathBuf>> {
        self.reverse.get(dependency)
    }

    /// Clear the entire dependency graph.
    #[inline]
    pub fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
    }

    // =========================================================================
    // Private helpers
    // =========================================================================

    /// Remove forward entry and clean up corresponding reverse mappings.
    fn remove_forward_entry(&mut self, content_file: &Path) {
        if let Some(old_deps) = self.forward.remove(content_file) {
            for dep in old_deps {
                if let Some(dependents) = self.reverse.get_mut(&dep) {
                    dependents.remove(content_file);
                    if dependents.is_empty() {
                        self.reverse.remove(&dep);
                    }
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn test_new_graph_has_no_dependents() {
        let graph = DependencyGraph::new();
        assert!(graph.get_dependents(&path("/any.typ")).is_none());
    }

    #[test]
    fn test_basic_dependency_recording() {
        let mut graph = DependencyGraph::new();

        let content = path("/project/content/index.typ");
        let template = path("/project/templates/base.typ");

        graph.record_dependencies(&content, &[template.clone()]);

        // Reverse lookup works
        let dependents = graph.get_dependents(&template).unwrap();
        assert!(dependents.contains(&content));
    }

    #[test]
    fn test_self_reference_excluded() {
        let mut graph = DependencyGraph::new();

        let content = path("/project/content/index.typ");
        let template = path("/project/templates/base.typ");

        // Include the content file itself in accessed_files (should be filtered out)
        graph.record_dependencies(&content, &[content.clone(), template.clone()]);

        // Self-reference should not appear in reverse lookup
        assert!(graph.get_dependents(&content).is_none());
        // Template dependency should exist
        assert!(graph.get_dependents(&template).unwrap().contains(&content));
    }

    #[test]
    fn test_dependency_update_replaces_old() {
        let mut graph = DependencyGraph::new();

        let content = path("/project/content/index.typ");
        let template1 = path("/project/templates/old.typ");
        let template2 = path("/project/templates/new.typ");

        // First: depends on template1
        graph.record_dependencies(&content, &[template1.clone()]);
        assert!(graph.get_dependents(&template1).is_some());

        // Second: switched to template2
        graph.record_dependencies(&content, &[template2.clone()]);

        // Old dependency should be cleaned up
        assert!(graph.get_dependents(&template1).is_none());
        // New dependency should exist
        assert!(graph.get_dependents(&template2).unwrap().contains(&content));
    }

    #[test]
    fn test_multiple_content_files_share_dependency() {
        let mut graph = DependencyGraph::new();

        let content1 = path("/project/content/a.typ");
        let content2 = path("/project/content/b.typ");
        let shared = path("/project/templates/shared.typ");

        graph.record_dependencies(&content1, &[shared.clone()]);
        graph.record_dependencies(&content2, &[shared.clone()]);

        let dependents = graph.get_dependents(&shared).unwrap();
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&content1));
        assert!(dependents.contains(&content2));
    }

    #[test]
    fn test_clear() {
        let mut graph = DependencyGraph::new();

        let template = path("/templates/base.typ");
        graph.record_dependencies(&path("/a.typ"), &[template.clone()]);
        graph.record_dependencies(&path("/c.typ"), &[path("/d.typ")]);

        graph.clear();

        assert!(graph.get_dependents(&template).is_none());
    }

    #[test]
    fn test_multiple_dependencies_per_file() {
        let mut graph = DependencyGraph::new();

        let content = path("/content/index.typ");
        let deps = vec![
            path("/templates/base.typ"),
            path("/utils/helper.typ"),
            path("/utils/date.typ"),
        ];

        graph.record_dependencies(&content, &deps);

        for dep in &deps {
            assert!(graph.get_dependents(dep).unwrap().contains(&content));
        }
    }

    #[test]
    fn test_empty_dependencies() {
        let mut graph = DependencyGraph::new();

        let content = path("/content/index.typ");

        // Record with no dependencies - should not create any reverse mappings
        graph.record_dependencies(&content, &[]);

        // No dependencies recorded
        assert!(graph.get_dependents(&content).is_none());
    }

    #[test]
    fn test_get_nonexistent_returns_none() {
        let graph = DependencyGraph::new();
        assert!(graph.get_dependents(&path("/nonexistent.typ")).is_none());
    }
}
