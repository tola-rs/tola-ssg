//! File system watcher for live reload.
//!
//! Monitors content, asset, template directories and config file for changes
//! and triggers rebuilds accordingly.
//!
//! # Relationship with `compiler/watch.rs`
//!
//! - **This module** (`src/watch.rs`): Event loop, debouncing, rebuild strategy
//! - **`compiler/watch.rs`**: Actual file compilation via [`process_watched_files`]
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │                      Event Loop                              │
//! │                                                              │
//! │  ┌──────────┐    ┌──────────┐    ┌────────────────────────┐  │
//! │  │ notify   │───▶│ Debouncer│───▶│    handle_changes()    │  │
//! │  │ events   │    │ (300ms)  │    │                        │  │
//! │  └──────────┘    └──────────┘    │  ┌──────────────────┐  │  │
//! │                                  │  │ Full Rebuild     │  │  │
//! │                                  │  │ (template/config)│  │  │
//! │                                  │  └──────────────────┘  │  │
//! │                                  │  ┌──────────────────┐  │  │
//! │                                  │  │ Incremental      │  │  │
//! │                                  │  │ (content/assets) │  │  │
//! │                                  │  └──────────────────┘  │  │
//! │                                  └────────────────────────┘  │
//! └──────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    compiler::process_watched_files,
    config::{cfg, reload_config, SiteConfig},
    log,
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
    FileCategory::Template,
    FileCategory::Utils,
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

/// Format path as relative without extension for log display.
///
/// `/proj/content/index.typ` → `content/index`
fn rel_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .with_extension("")
        .display()
        .to_string()
}

/// Log a build failure with error details.
fn log_build_error(kind: &str, trigger: &str, err: &anyhow::Error) {
    match (kind.is_empty(), trigger.is_empty()) {
        (true, true) => log!("watch"; "build failed"),
        (true, false) => log!("watch"; "build failed ({trigger})"),
        (false, true) => log!("watch"; "{kind} build failed"),
        (false, false) => log!("watch"; "{kind} build failed ({trigger})"),
    }
    log!("watch"; "{err}");
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

// =============================================================================
// Event Handler
// =============================================================================

/// Attempt a full site rebuild, logging errors on failure.
/// Clears the dependency graph before rebuilding to ensure fresh state.
/// Returns true if successful (for cooldown tracking).
fn try_full_rebuild(reason: &str) -> bool {
    use crate::compiler::deps::DEPENDENCY_GRAPH;

    log!("watch"; "{reason}");

    // Clear dependency graph before full rebuild
    DEPENDENCY_GRAPH.write().clear();

    match crate::build::build_site(&cfg()) {
        Ok(_) => true,
        Err(e) => {
            log_build_error("full", "", &e);
            false
        }
    }
}

/// Process file changes. Returns true if full rebuild succeeded (for cooldown).
fn handle_changes(paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }

    let c = cfg();
    let root = c.get_root().to_path_buf();
    let rel = |p: &Path| rel_path(p, &root);

    // Categorize changed files
    let mut config_changed = false;
    // Templates/utils: will query dependency graph, fallback to full rebuild if no deps cached
    let mut dependency_triggers: Vec<&PathBuf> = Vec::new();
    // Content/assets: always use incremental build
    let mut incremental_targets: Vec<PathBuf> = Vec::new();

    for path in paths {
        match categorize_path(path, &c) {
            FileCategory::Config => config_changed = true,
            FileCategory::Template | FileCategory::Utils => dependency_triggers.push(path),
            FileCategory::Content | FileCategory::Asset => incremental_targets.push(path.clone()),
            FileCategory::Unknown => {}
        }
    }

    // Config changes: reload config then full rebuild
    if config_changed {
        if let Err(e) = reload_config() {
            log!("watch"; "config reload failed: {e}");
            return false;
        }
        return try_full_rebuild("config changed, rebuilding...");
    }

    // Template/utils changes: query dependency graph for precise rebuild
    // Only falls back to full rebuild when no cached dependencies exist
    if !dependency_triggers.is_empty() {
        let affected = collect_affected_content(&dependency_triggers);

        if affected.is_empty() {
            // No known dependents - fall back to full rebuild
            let trigger = rel(dependency_triggers[0]);
            return try_full_rebuild(&format!("{trigger} changed (no deps cached), rebuilding..."));
        }

        // Log and add affected files to rebuild list
        log!("watch"; "{} changed, rebuilding {} affected files",
             dependency_triggers.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", "),
             affected.len());
        incremental_targets.extend(affected);
    }

    // Incremental build (content/assets)
    // Clean rebuild when triggered by dependency changes (templates/utils)
    let clean = !dependency_triggers.is_empty();
    let mut processed_content: FxHashSet<PathBuf> = FxHashSet::default();

    if !incremental_targets.is_empty() {
        // Track which content files we're about to process
        for path in &incremental_targets {
            if path.extension().is_some_and(|e| e == "typ") {
                processed_content.insert(path.clone());
            }
        }

        match process_watched_files(&incremental_targets, &cfg(), clean) {
            Ok(count) if count > 1 => {
                log!("watch"; "rebuilt {} files", count);
            }
            Ok(_) => {}
            Err(e) => {
                // When triggered by dependencies, show the trigger file(s), not all affected files
                let context = if clean {
                    dependency_triggers.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", ")
                } else {
                    incremental_targets.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", ")
                };
                log_build_error("", &context, &e);
                eprintln!(); // Blank line after error
                return false;
            }
        }
    }

    // After content changes, rebuild pages that depend on virtual data files
    // (e.g., pages using `json("/_data/tags.json")` for tag listings)
    if !processed_content.is_empty() {
        let virtual_dependents: Vec<PathBuf> = collect_virtual_data_dependents()
            .into_iter()
            .filter(|p| !processed_content.contains(p))
            .collect();

        if !virtual_dependents.is_empty() {
            log!("watch"; "updating {} pages using site data", virtual_dependents.len());
            // Use clean=false since templates haven't changed, only data
            if let Err(e) = process_watched_files(&virtual_dependents, &cfg(), false) {
                log_build_error("", "virtual data dependents", &e);
            }
        }
    }

    // Evict stale entries from typst's comemo memoization cache.
    // This prevents unbounded memory growth in long-running watch mode.
    typst::comemo::evict(COMEMO_CACHE_MAX_AGE);

    eprintln!(); // Blank line to separate rebuild sessions
    false
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

    // Dependency triggers: templates, utils, config file
    // Changes here trigger rebuild of dependent content files
    let dep_paths: Vec<_> = [
        (&build.templates, true),
        (&build.utils, true),
        (&config.config_path, false),
    ]
    .into_iter()
    .filter(|(p, _)| p.exists())
    .map(|(p, is_dir)| format_rel(p, root, is_dir))
    .collect();

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
        if let Some(path) = cat.path(config)
            && path.exists()
        {
            let mode = if cat.is_directory() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };

            watcher
                .watch(&path, mode)
                .with_context(|| format!("Failed to watch {}: {}", cat.name(), path.display()))?;
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

    loop {
        match rx.recv_timeout(debouncer.timeout()) {
            Ok(Ok(event)) if is_relevant(&event) && !debouncer.in_cooldown() => {
                debouncer.add(event);
            }
            Ok(Err(e)) => log!("watch"; "error: {e}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if debouncer.ready() => {
                if handle_changes(&debouncer.take()) {
                    debouncer.mark_rebuild();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            // Other cases: irrelevant events, timeout without ready, etc.
            _ => {}
        }
    }

    Ok(())
}
