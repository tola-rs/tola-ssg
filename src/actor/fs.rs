//! FileSystem Actor
//!
//! Watches for file changes and sends debounced events to the CompilerActor.
//! Implements the "Watcher-First" pattern for zero event loss.
//!
//! # Responsibility
//!
//! This actor is a **thin wrapper** that:
//! - Receives file system events from notify
//! - Debounces rapid changes
//! - Delegates classification to `pipeline::classify`
//! - Routes messages to CompilerActor

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use super::messages::CompilerMsg;
use crate::config::SiteConfig;
use crate::reload::classify::{ClassifyResult, classify_changes};
use crate::utils::path::normalize_path;

/// Debounce configuration
const DEBOUNCE_MS: u64 = 300;
const REBUILD_COOLDOWN_MS: u64 = 800;

/// Check if path is a temp/backup file (editor artifacts)
fn is_temp_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    matches!(ext, "bck" | "bak" | "backup" | "swp" | "swo" | "tmp")
        || name.ends_with('~')
        || name.starts_with('.')
}

/// FileSystem Actor - watches for file changes
pub struct FsActor {
    /// Channel to receive notify events (sync -> async bridge)
    notify_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Watcher handle (must be kept alive)
    _watcher: RecommendedWatcher,
    /// Channel to send messages to CompilerActor
    compiler_tx: mpsc::Sender<CompilerMsg>,
    /// Debouncer state
    debouncer: Debouncer,
    /// Site configuration for file classification
    config: Arc<SiteConfig>,
}

impl FsActor {
    /// Create a new FsActor with Watcher-First pattern
    ///
    /// The watcher starts immediately, buffering events while the caller
    /// performs initial build. This eliminates the "vacuum period".
    #[rustfmt::skip]
    pub fn new(
        paths: Vec<PathBuf>,
        compiler_tx: mpsc::Sender<CompilerMsg>,
        config: Arc<SiteConfig>,
    ) -> notify::Result<Self> {
        // Create sync channel for notify (it doesn't support async)
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();

        // Create and configure watcher IMMEDIATELY
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = notify_tx.send(res);
        })?;

        // Start watching all paths (skip non-existent paths to handle race conditions)
        for path in &paths {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        }

        // Events are now buffering in notify_rx while caller does initial build

        Ok(Self {
            notify_rx,
            _watcher: watcher,
            compiler_tx,
            debouncer: Debouncer::new(),
            config,
        })
    }

    /// Run the actor event loop
    pub async fn run(self) {
        // Extract fields before consuming self
        let notify_rx = self.notify_rx;
        let compiler_tx = self.compiler_tx;
        let config = self.config;
        let mut debouncer = self.debouncer;

        let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<notify::Event>(64);

        // Spawn a thread to poll notify events and send to async channel
        std::thread::spawn(move || {
            while let Ok(result) = notify_rx.recv() {
                match result {
                    Ok(event) => {
                        if async_tx.blocking_send(event).is_err() {
                            break; // Receiver dropped
                        }
                    }
                    Err(e) => crate::log!("watch"; "notify error: {}", e),
                }
            }
        });

        loop {
            tokio::select! {
                Some(event) = async_rx.recv() => debouncer.add_event(&event),
                _ = tokio::time::sleep(Duration::from_millis(50)) => if process_changes(&mut debouncer, &compiler_tx, &config).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Process debounced file changes
///
/// Returns `Err(())` if CompilerActor shut down
async fn process_changes(
    debouncer: &mut Debouncer,
    compiler_tx: &mpsc::Sender<CompilerMsg>,
    config: &SiteConfig,
) -> Result<(), ()> {
    let Some(paths) = debouncer.take_if_ready() else {
        return Ok(());
    };

    // Classify all changed files
    let result = classify_changes(&paths, config);

    // Collect all changed paths (for running watched hooks)
    let changed_paths: Vec<PathBuf> = result
        .classified
        .iter()
        .map(|(p, _)| p.clone())
        .collect();

    // If initial build failed, trigger full rebuild on any file change
    if !crate::core::is_healthy() {
        if changed_paths.is_empty() {
            return Ok(());
        }
        crate::log!("watch"; "retrying full build after change: {:?}", changed_paths);
        compiler_tx
            .send(CompilerMsg::FullRebuild)
            .await
            .map_err(|_| ())?;
        return Ok(());
    }

    // Log changes first (before consuming result)
    log_changes(&result);

    // When healthy: process all changes for hot-reload
    let messages = result_to_messages(result, changed_paths);
    if messages.is_empty() {
        return Ok(()); // Nothing to do
    }

    // Route to CompilerActor
    for msg in messages {
        compiler_tx.send(msg).await.map_err(|_| ())?;
    }

    Ok(())
}

/// Log file changes in verbose mode
fn log_changes(result: &ClassifyResult) {
    for (path, category) in &result.classified {
        crate::debug!("watch"; "{} changed: {}", category.name(), path.display());
    }
    if let Some(note) = &result.note {
        crate::log!("watch"; "{}", note);
    }
}

/// Convert ClassifyResult to CompilerMsg(s)
///
/// May return multiple messages:
/// - Compile for content files (with changed_paths for hooks)
/// - FullRebuild for config changes (overrides everything)
fn result_to_messages(result: ClassifyResult, changed_paths: Vec<PathBuf>) -> Vec<CompilerMsg> {
    if result.config_changed {
        return vec![CompilerMsg::FullRebuild];
    }

    let mut messages = Vec::new();

    // Asset changes
    if !result.asset_changed.is_empty() {
        messages.push(CompilerMsg::AssetChange(result.asset_changed));
    }

    // Content compilation (with changed_paths for hooks)
    if !result.compile_queue.is_empty() {
        messages.push(CompilerMsg::Compile {
            queue: result.compile_queue,
            changed_paths,
        });
    }

    messages
}

/// Simple debouncer for file events
struct Debouncer {
    /// Accumulated changed paths
    changed: Vec<PathBuf>,
    /// Time of last event
    last_event: Option<std::time::Instant>,
    /// Time of last compile
    last_compile: Option<std::time::Instant>,
}

impl Debouncer {
    fn new() -> Self {
        Self {
            changed: Vec::new(),
            last_event: None,
            last_compile: None,
        }
    }

    fn add_event(&mut self, event: &notify::Event) {
        self.last_event = Some(std::time::Instant::now());

        for path in &event.paths {
            // Skip editor temporary/backup files
            if is_temp_file(path) {
                continue;
            }

            // Normalize path to ensure consistent keys with VDOM cache
            // Fixes macOS /var vs /private/var symlink issues
            let path = normalize_path(path);

            if !self.changed.contains(&path) {
                self.changed.push(path);
            }
        }
    }

    /// Take paths if ready and build is complete.
    ///
    /// Returns `None` if:
    /// - Debounce period not elapsed
    /// - Cooldown from last compile not elapsed
    /// - Initial scan not complete (not serving)
    /// - No existing files to process (deleted files and directories are filtered out)
    fn take_if_ready(&mut self) -> Option<Vec<PathBuf>> {
        if !self.is_ready() {
            return None;
        }

        // Wait for initial scan to complete before processing changes
        // This prevents false "deps changed but no dependents" when
        // dependency graph hasn't been populated yet
        // Note: We check is_serving() not is_healthy() to allow hot-reload
        // during on-demand compilation (scheduler mode)
        if !crate::core::is_serving() {
            return None;
        }

        let paths = self.take();
        if paths.is_empty() {
            return None;
        }

        Some(paths)
    }

    fn is_ready(&self) -> bool {
        let Some(last_event) = self.last_event else {
            return false;
        };

        // Must wait for debounce period
        if last_event.elapsed() < Duration::from_millis(DEBOUNCE_MS) {
            return false;
        }

        // Must wait for cooldown from last compile
        if let Some(last_compile) = self.last_compile
            && last_compile.elapsed() < Duration::from_millis(REBUILD_COOLDOWN_MS)
        {
            return false;
        }

        !self.changed.is_empty()
    }

    fn take(&mut self) -> Vec<PathBuf> {
        self.last_event = None;
        self.last_compile = Some(std::time::Instant::now());
        std::mem::take(&mut self.changed)
            .into_iter()
            .filter(|p| p.exists() && p.is_file()) // Filter out directories
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debouncer_empty() {
        let debouncer = Debouncer::new();
        assert!(!debouncer.is_ready());
    }
}
