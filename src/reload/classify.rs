//! File Classification Pipeline
//!
//! Pure functions for classifying changed files and determining rebuild strategy.
//! No Actor machinery, no side effects.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;

// Re-export for convenience
use crate::compiler::dependency::get_dependents;
use crate::config::SiteConfig;
pub use crate::core::{ContentKind, FileCategory};
use crate::utils::path::normalize_path;

use super::active::ACTIVE_PAGE;
use super::queue::{CompileQueue, Priority};

// =============================================================================
// Classification
// =============================================================================

/// Categorize a path based on config directories
///
/// Note: The path should already be normalized before calling this function
/// Use `normalize_path()` on watcher paths before classification
pub fn categorize_path(path: &Path, config: &SiteConfig) -> FileCategory {
    // Check output directory first (hook-generated files)
    if path.starts_with(config.paths().output_dir()) {
        return FileCategory::Output;
    }
    if path == config.config_path {
        FileCategory::Config
    } else if config.build.deps.iter().any(|dep| path.starts_with(dep)) {
        FileCategory::Deps
    } else if path.starts_with(&config.build.content) {
        // Check extension to determine content kind
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ContentKind::from_extension(ext) {
            Some(kind) => FileCategory::Content(kind),
            None => FileCategory::Unknown, // Unsupported content type
        }
    } else if config.build.assets.contains_source(path) {
        FileCategory::Asset
    } else {
        FileCategory::Unknown
    }
}

/// Result of classifying changed files
#[derive(Debug)]
pub struct ClassifyResult {
    /// Files grouped by category (for logging)
    pub classified: Vec<(PathBuf, FileCategory)>,
    /// Config changed - requires full rebuild
    pub config_changed: bool,
    /// Prioritized compilation queue (content files)
    pub compile_queue: CompileQueue,
    /// Asset files that changed (need copy, not compile)
    pub asset_changed: Vec<PathBuf>,
    /// Output files that changed (from hooks, trigger hot reload)
    pub output_changed: Vec<PathBuf>,
    /// Optional note (e.g., "deps changed but no dependents")
    pub note: Option<String>,
}

impl ClassifyResult {
    /// Get files to compile in priority order (consumes queue).
    #[allow(dead_code)]
    pub fn files_to_compile(self) -> Vec<PathBuf> {
        self.compile_queue.into_ordered()
    }
}

/// Classify changed files and determine rebuild strategy
///
/// This is a pure function that:
/// - Categorizes each file
/// - Resolves dependency relationships
/// - Returns prioritized compilation queue
pub fn classify_changes(paths: &[PathBuf], config: &SiteConfig) -> ClassifyResult {
    let mut classified = Vec::new();
    let mut config_changed = false;
    let mut deps_changed = Vec::new();
    let mut content_changed = Vec::new();
    let mut asset_changed = Vec::new();
    let mut output_changed = Vec::new();

    // Categorize each path
    for path in paths {
        // Normalize path for consistent matching with config paths
        // This is critical for assets: the watcher may send non-canonicalized paths
        // when files are newly created, but config paths are canonicalized.
        let normalized = normalize_path(path);
        let category = categorize_path(&normalized, config);
        classified.push((normalized.clone(), category));

        match category {
            FileCategory::Config => config_changed = true,
            FileCategory::Deps => deps_changed.push(normalized),
            FileCategory::Content(_) => content_changed.push(normalized),
            FileCategory::Asset => asset_changed.push(normalized),
            FileCategory::Output => output_changed.push(normalized),
            FileCategory::Unknown => {}
        }
    }

    // Build prioritized compile queue
    let mut note = None;
    let mut queue = CompileQueue::new();

    if !config_changed {
        // Add directly modified content files (Priority::Direct)
        queue.add(content_changed.clone(), Priority::Direct);

        // If deps changed, add affected files (Priority::Affected)
        if !deps_changed.is_empty() {
            let affected = collect_dependents(&deps_changed);
            if affected.is_empty() {
                note = Some("deps changed but no dependents found".to_string());
            } else {
                // Add affected files that weren't directly changed
                let direct_set: FxHashSet<_> = content_changed.into_iter().collect();
                let affected_only: Vec<_> = affected
                    .into_iter()
                    .filter(|p| !direct_set.contains(p))
                    .collect();
                queue.add(affected_only, Priority::Affected);
            }
        }

        // Check if there are active pages that should be prioritized
        for active_url in ACTIVE_PAGE.get_all() {
            // Convert URL to file path and set as active priority
            if let Some(active_path) = url_to_content_path(active_url.as_str(), config) {
                queue.set_active(active_path);
            }
        }
    }

    // If deps changed but no dependents, treat as config change
    let config_changed =
        config_changed || (!deps_changed.is_empty() && queue.is_empty() && note.is_some());

    ClassifyResult {
        classified,
        config_changed,
        compile_queue: queue,
        asset_changed,
        output_changed,
        note,
    }
}

/// Convert a URL path to content file path using AddressSpace
///
/// Uses the same path source as the dependency graph (PageRoute.source),
/// ensuring consistent path matching in set_active()
pub fn url_to_content_path(url: &str, _config: &SiteConfig) -> Option<PathBuf> {
    use crate::address::GLOBAL_ADDRESS_SPACE;
    use crate::core::UrlPath;

    let url_path = UrlPath::from_page(url);
    let space = GLOBAL_ADDRESS_SPACE.read();

    space
        .get_by_url(&url_path)
        .map(|resource| resource.source().to_path_buf())
}

/// Collect all content files that depend on the changed files
pub fn collect_dependents(changed_files: &[PathBuf]) -> Vec<PathBuf> {
    let mut affected = FxHashSet::default();

    for path in changed_files {
        affected.extend(get_dependents(path.as_path()));
    }

    affected.into_iter().collect()
}
