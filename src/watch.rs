//! File system watcher for live reload.
//!
//! Monitors content, asset, template directories and config file for changes
//! and triggers rebuilds accordingly.

use crate::{
    config::SiteConfig,
    log,
    utils::watch::{ChangeType, process_watched_files},
};
use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Debounce duration in milliseconds to prevent duplicate events
const DEBOUNCE_MS: u64 = 300;

/// Cooldown duration after full rebuild to prevent loops
const FULL_REBUILD_COOLDOWN_MS: u64 = 1000;

/// Start blocking file watcher for content and asset changes
pub fn watch_for_changes_blocking(
    config: &'static SiteConfig,
    server_ready: Arc<AtomicBool>,
) -> Result<()> {
    if !config.serve.watch {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("Failed to create file watcher")?;

    // All paths are already absolute from config
    watch_directory(&mut watcher, "content", &config.build.content)?;
    watch_directory(&mut watcher, "assets", &config.build.assets)?;

    // Watch templates and utils directories (for full rebuild)
    if config.build.templates.exists() {
        watch_directory(&mut watcher, "templates", &config.build.templates)?;
    }
    if config.build.utils.exists() {
        watch_directory(&mut watcher, "utils", &config.build.utils)?;
    }

    // Watch config file
    if config.config_path.exists() {
        watch_file(&mut watcher, "config", &config.config_path)?;
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
                if !server_ready.load(Ordering::Relaxed) {
                    break;
                }

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

/// Watch a directory and log the action
fn watch_directory(watcher: &mut impl Watcher, name: &str, path: &Path) -> Result<()> {
    watcher
        .watch(path, RecursiveMode::Recursive)
        .with_context(|| format!("Failed to watch {name} directory: {}", path.display()))?;
    log!("watch"; "watching for changes in {}: {}", name, path.display());
    Ok(())
}

/// Watch a single file and log the action
fn watch_file(watcher: &mut impl Watcher, name: &str, path: &Path) -> Result<()> {
    watcher
        .watch(path, RecursiveMode::NonRecursive)
        .with_context(|| format!("Failed to watch {name} file: {}", path.display()))?;
    log!("watch"; "watching for changes in {}: {}", name, path.display());
    Ok(())
}

/// Determine if an event should trigger a rebuild
fn should_process_event(event: &Event) -> bool {
    matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
}

/// Classify file change type based on path
fn classify_change(path: &Path, config: &SiteConfig) -> ChangeType {
    // Canonicalize the incoming path for comparison
    // Config paths are already absolute/canonicalized
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if path == config.config_path
        || path.starts_with(&config.build.templates)
        || path.starts_with(&config.build.utils)
    {
        ChangeType::FullRebuild
    } else if path.starts_with(&config.build.content) {
        ChangeType::Content
    } else if path.starts_with(&config.build.assets) {
        ChangeType::Asset
    } else {
        ChangeType::Unknown
    }
}

/// Handle file change events, returns true if full rebuild was performed
fn handle_event(paths: &[std::path::PathBuf], config: &'static SiteConfig) -> bool {
    // Classify all paths and find which triggered full rebuild
    let rebuild_trigger = paths
        .iter()
        .find(|p| matches!(classify_change(p, config), ChangeType::FullRebuild));

    if let Some(trigger_path) = rebuild_trigger {
        let reason = get_rebuild_reason(trigger_path, config);
        log!("watch"; "{reason} changed, triggering full rebuild...");
        if let Err(err) = crate::build::build_site(config, true) {
            log!("watch"; "full rebuild failed: {err}");
        }
        return true;
    }

    // Process incremental changes
    if let Err(err) =
        process_watched_files(paths, config).context("Failed to process changed files")
    {
        log!("watch"; "{err}");
    }
    false
}

/// Get a human-readable reason for the rebuild trigger
fn get_rebuild_reason(path: &Path, config: &SiteConfig) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if path == config.config_path {
        let config_name = config
            .config_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config");
        format!("config ({config_name})")
    } else if path.starts_with(&config.build.templates) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        format!("template ({file_name})")
    } else if path.starts_with(&config.build.utils) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        format!("utils ({file_name})")
    } else {
        "unknown".to_string()
    }
}
