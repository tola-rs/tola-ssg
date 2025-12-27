//! File system watcher for live reload.
//!
//! Monitors content, asset, template, utils directories and config file for
//! changes, triggering rebuilds accordingly.
//!
//! # Relationship with `compiler/watch.rs`
//!
//! - **This module** (`src/watch.rs`): Event loop, debouncing, rebuild strategy
//! - **`compiler/watch.rs`**: Actual file compilation via [`process_watched_files`]
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         Event Loop                                  │
//! │                                                                     │
//! │  ┌────────┐   ┌──────────┐   ┌──────────────┐   ┌───────────────┐   │
//! │  │ notify │──▶│ Debouncer│──▶│ ContentCache │──▶│handle_changes │   │
//! │  │ events │   │ (300ms)  │   │ (hash check) │   │               │   │
//! │  └────────┘   └──────────┘   └──────────────┘   │ ┌───────────┐ │   │
//! │                                                 │ │ Dependent │ │   │
//! │                 unchanged ─────────────────────▶│ │ (template │ │   │
//! │                    │                            │ │  utils    │ │   │
//! │               WatchStatus                       │ │  config)  │ │   │
//! │               "unchanged"                       │ └───────────┘ │   │
//! │                                                 │ ┌───────────┐ │   │
//! │                                                 │ │Incremental│ │   │
//! │                                                 │ │ (content  │ │   │
//! │                                                 │ │  assets)  │ │   │
//! │                                                 │ └───────────┘ │   │
//! │                                                 └───────────────┘   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    compiler::process_watched_files,
    config::{cfg, reload_config, SiteConfig},
    log,
    logger::WatchStatus,
    utils::category::{categorize_path, FileCategory},
};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use rustc_hash::FxHashSet;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

// =============================================================================
// Constants
// =============================================================================

const DEBOUNCE_MS: u64 = 300;
const REBUILD_COOLDOWN_MS: u64 = 800;

/// Maximum age for comemo cache entries before eviction.
/// Entries unused for this many compilations will be removed.
const COMEMO_CACHE_MAX_AGE: usize = 30;

const WATCH_CATEGORIES: &[FileCategory] = &[
    FileCategory::Content,
    FileCategory::Asset,
    FileCategory::Deps,
    FileCategory::Config,
];

// =============================================================================
// Path Utilities
// =============================================================================

/// Check if path is a temp/backup file (editor artifacts).
fn is_temp_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    matches!(ext, "bck" | "bak" | "backup" | "swp" | "swo" | "tmp")
        || name.ends_with('~')
        || name.starts_with('.')
}

// =============================================================================
// Content Cache (detects actual content changes via hashing)
// =============================================================================

/// Result of filtering paths through content cache.
struct FilterResult {
    /// Files with actual content changes
    changed: Vec<PathBuf>,
    /// Files touched but content unchanged
    unchanged: Vec<PathBuf>,
}

/// Tracks file content hashes to skip rebuilds when files are touched but unchanged.
///
/// Separate from `Debouncer` which handles time-based event batching.
/// This handles content-based change detection.
struct ContentCache {
    hashes: rustc_hash::FxHashMap<PathBuf, u64>,
}

impl ContentCache {
    fn new() -> Self {
        Self {
            hashes: rustc_hash::FxHashMap::default(),
        }
    }

    /// Pre-populate hashes for all watched files.
    ///
    /// Called at startup to establish baseline, so first touch without
    /// content change won't trigger rebuild.
    fn populate(&mut self, config: &SiteConfig) {
        use walkdir::WalkDir;

        for &cat in WATCH_CATEGORIES {
            for path in cat.paths(config) {
                if !path.exists() { continue; }

                for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_file() && !is_temp_file(path) {
                        if let Ok(file) = std::fs::File::open(path) {
                            if let Ok(hash) = crate::utils::hash::compute_reader(file) {
                                self.hashes.insert(path.to_path_buf(), hash);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Filter paths into changed and unchanged.
    ///
    /// - Deleted files go into `changed` (and removed from cache)
    /// - Files with different content go into `changed`
    /// - Files with identical content go into `unchanged`
    fn filter(&mut self, paths: &[PathBuf]) -> FilterResult {
        let mut changed = Vec::new();
        let mut unchanged = Vec::new();

        for path in paths {
            if !path.exists() {
                // File deleted - always process, remove from cache
                self.hashes.remove(path);
                changed.push(path.clone());
                continue;
            }

            // Compute current hash
            let Ok(file) = std::fs::File::open(path) else {
                // Can't open - assume changed to be safe
                changed.push(path.clone());
                continue;
            };

            let Ok(hash) = crate::utils::hash::compute_reader(file) else {
                // Read failed - assume changed
                changed.push(path.clone());
                continue;
            };

            // Compare with cached hash
            if self.hashes.get(path) == Some(&hash) {
                unchanged.push(path.clone());
                continue;
            }

            // Update cache and include in changed list
            self.hashes.insert(path.clone(), hash);
            changed.push(path.clone());
        }

        FilterResult { changed, unchanged }
    }
}

// =============================================================================
// Debounce State
// =============================================================================

/// Batches rapid file events with debouncing and rebuild cooldown.
struct Debouncer {
    pending: FxHashSet<PathBuf>,
    last_event: Option<Instant>,
    last_rebuild: Option<Instant>,
}

impl Debouncer {
    fn new() -> Self {
        Self {
            pending: FxHashSet::default(),
            last_event: None,
            last_rebuild: None,
        }
    }

    fn in_cooldown(&self) -> bool {
        self.last_rebuild
            .is_some_and(|t| t.elapsed() < Duration::from_millis(REBUILD_COOLDOWN_MS))
    }

    fn add(&mut self, event: Event) {
        for path in event.paths {
            if !is_temp_file(&path) {
                self.pending.insert(path);
            }
        }
        self.last_event = Some(Instant::now());
    }

    fn ready(&self) -> bool {
        !self.pending.is_empty()
            && self
                .last_event
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(DEBOUNCE_MS))
    }

    fn take(&mut self) -> Vec<PathBuf> {
        self.last_event = None;
        self.pending.drain().collect()
    }

    fn mark_rebuild(&mut self) {
        self.last_rebuild = Some(Instant::now());
    }

    fn timeout(&self) -> Duration {
        if self.pending.is_empty() {
            Duration::from_secs(60)
        } else {
            Duration::from_millis(DEBOUNCE_MS)
        }
    }
}


/// Process file changes. Returns true if full rebuild succeeded (for cooldown).
fn handle_changes(paths: &[PathBuf], status: &mut WatchStatus, root: &Path) -> bool {
    if paths.is_empty() {
        return false;
    }

    let c = cfg();
    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .with_extension("")
            .display()
            .to_string()
    };

    // Categorize changed files
    let mut config_changed = false;
    let mut dependency_triggers: Vec<&PathBuf> = Vec::new();
    let mut incremental_targets: Vec<PathBuf> = Vec::new();

    for path in paths {
        match categorize_path(path, &c) {
            FileCategory::Config => config_changed = true,
            FileCategory::Deps => dependency_triggers.push(path),
            FileCategory::Content | FileCategory::Asset => incremental_targets.push(path.clone()),
            FileCategory::Unknown => {}
        }
    }

    // Config changes: reload config then full rebuild
    if config_changed {
        if let Err(e) = reload_config() {
            status.error("config reload failed", &e.to_string());
            return false;
        }
        return handle_full_rebuild("config changed", status);
    }

    // Template/utils changes: query dependency graph for precise rebuild
    if !dependency_triggers.is_empty() {
        let affected = collect_affected_content(&dependency_triggers);

        if affected.is_empty() {
            let trigger = rel(dependency_triggers[0]);
            return handle_full_rebuild(&format!("{trigger} (no deps cached)"), status);
        }

        incremental_targets.extend(affected);
    }

    // Incremental build (content/assets)
    let clean = !dependency_triggers.is_empty();
    let mut processed_content: FxHashSet<PathBuf> = FxHashSet::default();

    if !incremental_targets.is_empty() {
        for path in &incremental_targets {
            if path.extension().is_some_and(|e| e == "typ") {
                processed_content.insert(path.clone());
            }
        }

        match process_watched_files(&incremental_targets, &cfg(), clean) {
            Ok(count) => {
                let msg = if count == 1 {
                    format!("rebuilt: {}", rel(&incremental_targets[0]))
                } else {
                    format!("rebuilt {} files", count)
                };
                status.success(&msg);
            }
            Err(e) => {
                let context = if clean {
                    dependency_triggers.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", ")
                } else {
                    incremental_targets.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", ")
                };
                status.error(&format!("failed: {context}"), &e.to_string());
                return false;
            }
        }
    }

    // Virtual data dependents
    if !processed_content.is_empty() {
        let virtual_dependents: Vec<PathBuf> = collect_virtual_data_dependents()
            .into_iter()
            .filter(|p| !processed_content.contains(p))
            .collect();

        if !virtual_dependents.is_empty() {
            if let Err(e) = process_watched_files(&virtual_dependents, &cfg(), false) {
                status.error("failed: site data update", &e.to_string());
            }
        }
    }

    // Evict stale entries from typst's comemo memoization cache
    typst::comemo::evict(COMEMO_CACHE_MAX_AGE);

    false
}

/// Helper for full rebuild with status output.
fn handle_full_rebuild(reason: &str, status: &mut WatchStatus) -> bool {
    use crate::compiler::deps::DEPENDENCY_GRAPH;

    DEPENDENCY_GRAPH.write().clear();

    match crate::build::build_site(&cfg(), true) {
        Ok(_) => {
            status.success(&format!("full rebuild: {reason}"));
            true
        }
        Err(e) => {
            status.error(&format!("full rebuild failed: {reason}"), &e.to_string());
            false
        }
    }
}

/// Collect all content files affected by template/utils changes.
///
/// Note: All entries in the dependency graph's reverse map are content files
/// (only content files call `record_dependencies`), so no category check needed.
fn collect_affected_content(changed_files: &[&PathBuf]) -> Vec<PathBuf> {
    use crate::compiler::deps::DEPENDENCY_GRAPH;

    let graph = DEPENDENCY_GRAPH.read();
    let mut affected = FxHashSet::default();

    for path in changed_files {
        if let Some(dependents) = graph.get_dependents(path) {
            affected.extend(dependents.iter().cloned());
        }
    }

    affected.into_iter().collect()
}

/// Collect all content files that depend on virtual data files.
///
/// Virtual data files (`/_data/tags.json`, `/_data/pages.json`) are updated
/// when any content file changes. Pages that read these files need to be
/// rebuilt to reflect the updated data.
fn collect_virtual_data_dependents() -> Vec<PathBuf> {
    use crate::compiler::deps::DEPENDENCY_GRAPH;
    use crate::data::virtual_fs;

    let graph = DEPENDENCY_GRAPH.read();
    let mut affected = FxHashSet::default();

    // Check for dependents of both virtual data files
    for virtual_path in virtual_fs::virtual_data_paths() {
        if let Some(dependents) = graph.get_dependents(&virtual_path) {
            affected.extend(dependents.iter().cloned());
        }
    }

    affected.into_iter().collect()
}

// =============================================================================
// Watcher Setup
// =============================================================================

/// Format absolute path as relative to root, with trailing slash for directories.
fn format_rel(path: &Path, root: &Path, is_dir: bool) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let suffix = if is_dir { "/" } else { "" };
    format!("{}{}", rel.display(), suffix)
}

/// Log watched paths grouped by rebuild strategy.
fn log_watch_summary(config: &SiteConfig) {
    let root = config.get_root();
    let build = &config.build;

    // Dependency triggers: deps directories + config file
    // Changes here trigger rebuild of dependent content files
    let mut dep_paths: Vec<_> = build
        .deps
        .iter()
        .filter(|p| p.exists())
        .map(|p| format_rel(p, root, true))
        .collect();
    if config.config_path.exists() {
        dep_paths.push(format_rel(&config.config_path, root, false));
    }

    // Incremental triggers: content, assets
    let incr_paths: Vec<_> = [(&build.content, true), (&build.assets, true)]
        .into_iter()
        .filter(|(p, _)| p.exists())
        .map(|(p, is_dir)| format_rel(p, root, is_dir))
        .collect();

    if !dep_paths.is_empty() {
        log!("watch"; "dependent: {}", dep_paths.join(", "));
    }
    if !incr_paths.is_empty() {
        log!("watch"; "incremental: {}", incr_paths.join(", "));
    }
}

fn setup_watchers(watcher: &mut impl Watcher, config: &SiteConfig) -> Result<()> {
    for &cat in WATCH_CATEGORIES {
        let mode = if cat.is_directory() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        for path in cat.paths(config) {
            if path.exists() {
                watcher
                    .watch(&path, mode)
                    .with_context(|| format!("Failed to watch {}: {}", cat.name(), path.display()))?;
            }
        }
    }

    log_watch_summary(config);
    eprintln!(); // Blank line to separate init logs from change events
    Ok(())
}

const fn is_relevant(event: &Event) -> bool {
    matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
}


// =============================================================================
// Public API
// =============================================================================

/// Start blocking file watcher with debouncing and live rebuild.
pub fn watch_for_changes_blocking() -> Result<()> {
    let c = cfg();
    if !c.serve.watch {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("Failed to create file watcher")?;
    setup_watchers(&mut watcher, &c)?;

    let mut debouncer = Debouncer::new();
    let mut content_cache = ContentCache::new();
    let mut status = WatchStatus::new();
    content_cache.populate(&c);

    let root = c.get_root().to_path_buf();

    loop {
        match rx.recv_timeout(debouncer.timeout()) {
            Ok(Ok(event)) if is_relevant(&event) && !debouncer.in_cooldown() => {
                debouncer.add(event);
            }
            Ok(Err(e)) => log!("watch"; "error: {e}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if debouncer.ready() => {
                let result = content_cache.filter(&debouncer.take());

                // Show unchanged files
                for path in &result.unchanged {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    status.unchanged(&rel.display().to_string());
                }

                // Process changed files
                if !result.changed.is_empty() {
                    if handle_changes(&result.changed, &mut status, &root) {
                        debouncer.mark_rebuild();
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
    }

    Ok(())
}
