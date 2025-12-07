//! File system watcher for live reload.
//!
//! Monitors content, asset, template directories and config file for changes
//! and triggers rebuilds accordingly.
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
    config::SiteConfig,
    log,
    utils::category::{categorize_path, FileCategory},
};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

// =============================================================================
// Constants
// =============================================================================

const DEBOUNCE_MS: u64 = 300;
const REBUILD_COOLDOWN_MS: u64 = 800;

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

/// Format path as relative without extension: `/proj/content/index.typ` → `content/index`
fn rel_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .with_extension("")
        .display()
        .to_string()
}

/// Log a build failure with error details.
fn log_build_error(kind: &str, trigger: &str, err: &anyhow::Error) {
    if trigger.is_empty() {
        log!("watch"; "{kind} build failed");
    } else {
        log!("watch"; "{kind} build failed ({trigger})");
    }
    log!("watch"; "{err}");
}

// =============================================================================
// Debounce State
// =============================================================================

/// Batches rapid file events with debouncing and rebuild cooldown.
struct Debouncer {
    pending: HashMap<String, PathBuf>,
    last_event: Option<Instant>,
    last_rebuild: Option<Instant>,
}

impl Debouncer {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
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
                self.pending.insert(path.to_string_lossy().into(), path);
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
        self.pending.drain().map(|(_, p)| p).collect()
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

/// Process file changes. Returns true if full rebuild succeeded (for cooldown).
fn handle_changes(paths: &[PathBuf], config: &'static SiteConfig) -> bool {
    if paths.is_empty() {
        return false;
    }

    let root = config.get_root();
    let rel = |p: &Path| rel_path(p, root);

    // Check for full rebuild trigger (template/utils/config)
    if let Some(trigger) = paths.iter().find(|p| categorize_path(p, config).requires_full_rebuild()) {
        log!("watch"; "{} changed, rebuilding...", rel(trigger));
        return match crate::build::build_site(config) {
            Ok(_) => true,
            Err(e) => {
                log_build_error("full", "", &e);
                false
            }
        };
    }

    // Incremental build (content/assets)
    if let Err(e) = process_watched_files(paths, config) {
        let files = paths.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", ");
        log_build_error("incremental", &files, &e);
    }
    false
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

    // Full rebuild triggers: templates, utils, config file
    let full_paths: Vec<_> = [
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

    if !full_paths.is_empty() {
        log!("watch"; "full: {}", full_paths.join(", "));
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
pub fn watch_for_changes_blocking(config: &'static SiteConfig) -> Result<()> {
    if !config.serve.watch {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("Failed to create file watcher")?;
    setup_watchers(&mut watcher, config)?;

    let mut debouncer = Debouncer::new();

    loop {
        match rx.recv_timeout(debouncer.timeout()) {
            Ok(Ok(event)) if is_relevant(&event) && !debouncer.in_cooldown() => {
                debouncer.add(event);
            }
            Ok(Err(e)) => log!("watch"; "error: {e}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if debouncer.ready() => {
                if handle_changes(&debouncer.take(), config) {
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
