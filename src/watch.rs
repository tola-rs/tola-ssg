//! File system watcher for live reload.
//!
//! Monitors content, asset, template directories and config file for changes
//! and triggers rebuilds accordingly.

use crate::{
    config::SiteConfig,
    log,
    utils::{
        category::{FileCategory, categorize_path},
        watch::process_watched_files,
    },
};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Debounce duration in milliseconds to prevent duplicate events
const DEBOUNCE_MS: u64 = 300;

/// Cooldown duration after full rebuild to prevent loops
const FULL_REBUILD_COOLDOWN_MS: u64 = 1000;

/// All file categories to watch
const WATCH_CATEGORIES: &[FileCategory] = &[
    FileCategory::Content,
    FileCategory::Asset,
    FileCategory::Template,
    FileCategory::Utils,
    FileCategory::Config,
];

/// Start blocking file watcher for content and asset changes.
pub fn watch_for_changes_blocking(config: &'static SiteConfig) -> Result<()> {
    if !config.serve.watch {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("Failed to create file watcher")?;

    // Register watchers for all categories
    for &category in WATCH_CATEGORIES {
        if let Some(path) = category.path(config)
            && path.exists()
        {
            let mode = if category.is_directory() {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            watcher.watch(&path, mode).with_context(|| {
                format!("Failed to watch {}: {}", category.name(), path.display())
            })?;
            log!("watch"; "watching for changes in {}: {}", category.name(), path.display());
        }
    }

    let debounce_duration = Duration::from_millis(DEBOUNCE_MS);
    let rebuild_cooldown = Duration::from_millis(FULL_REBUILD_COOLDOWN_MS);
    let mut pending_paths: HashMap<String, std::path::PathBuf> = HashMap::new();
    let mut last_event_time: Option<Instant> = None;
    let mut last_full_rebuild: Option<Instant> = None;

    loop {
        // Use timeout to allow debounce batching
        let timeout = if pending_paths.is_empty() {
            Duration::from_secs(60) // Long timeout when idle
        } else {
            debounce_duration
        };

        match rx.recv_timeout(timeout) {
            Ok(res) => {
                match res {
                    Err(e) => log!("watch"; "error: {e:?}"),
                    Ok(event) if should_process_event(&event) => {
                        let now = Instant::now();

                        // Skip all events during full rebuild cooldown
                        if let Some(rebuild_time) = last_full_rebuild
                            && now.duration_since(rebuild_time) < rebuild_cooldown
                        {
                            continue;
                        }

                        // Collect paths for batched processing
                        for path in event.paths {
                            let path_str = path.to_string_lossy().to_string();
                            pending_paths.insert(path_str, path);
                        }
                        last_event_time = Some(now);
                    }
                    _ => {}
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Process pending paths after debounce timeout
                if !pending_paths.is_empty()
                    && let Some(last_time) = last_event_time
                    && Instant::now().duration_since(last_time) >= debounce_duration
                {
                    let paths: Vec<_> = pending_paths.drain().map(|(_, p)| p).collect();
                    let did_full_rebuild = handle_event(&paths, config);
                    if did_full_rebuild {
                        last_full_rebuild = Some(Instant::now());
                    }
                    last_event_time = None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

/// Determine if an event should trigger a rebuild
fn should_process_event(event: &Event) -> bool {
    matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
}

/// Handle file change events, returns true if full rebuild was performed
fn handle_event(paths: &[std::path::PathBuf], config: &'static SiteConfig) -> bool {
    // Find if any changed file requires a full rebuild
    let rebuild_trigger = paths
        .iter()
        .map(|p| (p, categorize_path(p, config)))
        .find(|(_, cat)| cat.requires_full_rebuild());

    if let Some((trigger_path, category)) = rebuild_trigger {
        let reason = category.description(trigger_path);
        log!("watch"; "{reason} changed, triggering full rebuild...");
        // Full rebuild for template/utils/config changes, but no need to clean output
        if let Err(err) = crate::build::build_site(config) {
            log!("watch"; "full rebuild failed: {err}");
        }
        return true;
    }

    // Process incremental changes (content/asset files only)
    if let Err(err) =
        process_watched_files(paths, config).context("Failed to process changed files")
    {
        log!("watch"; "{err}");
    }
    false
}
