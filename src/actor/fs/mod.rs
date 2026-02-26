//! FileSystem Actor
//!
//! Watches for file changes and sends debounced events to the CompilerActor.
//! Implements the "Watcher-First" pattern for zero event loss.
//!
//! Architecture:
//! ```text
//! Watcher → Debouncer (pure timing) → Classifier (business logic) → CompilerMsg
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use notify::RecommendedWatcher;
use tokio::sync::mpsc;

use super::messages::CompilerMsg;
use crate::config::SiteConfig;

// Business classification pipeline (raw changes -> actionable events).
mod classifier;
// Pure timing and deduplication.
mod debouncer;
// Event routing (actionable events -> CompilerMsg).
mod router;
// Shared fs event types.
mod types;
// Watch root attach/re-attach lifecycle.
mod watch_roots;

#[cfg(test)]
mod tests;

use classifier::EventClassifier;
use debouncer::Debouncer;
use router::{events_to_messages, log_events};
use watch_roots::WatchRoots;

/// FileSystem Actor - watches for file changes
pub struct FsActor {
    /// Channel to receive notify events (sync -> async bridge)
    notify_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    /// Watcher handle (must be kept alive)
    watcher: RecommendedWatcher,
    /// Watch-root consistency layer (attach/re-attach root directories)
    watch_roots: WatchRoots,
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

        // Start watching all existing roots (missing roots will be re-attached later)
        let mut watch_roots = WatchRoots::new(paths);
        watch_roots.attach_existing(&mut watcher)?;

        // Events are now buffering in notify_rx while caller does initial build

        Ok(Self {
            notify_rx,
            watcher,
            watch_roots,
            compiler_tx,
            debouncer: Debouncer::new(),
            config,
        })
    }

    /// Run the actor event loop
    pub async fn run(self) {
        // Extract fields before consuming self
        let notify_rx = self.notify_rx;
        let compiler_tx = self.compiler_tx.clone();
        let config = Arc::clone(&self.config);
        let mut debouncer = self.debouncer;
        let mut watcher = self.watcher;
        let mut watch_roots = self.watch_roots;

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
                biased;
                Some(event) = async_rx.recv() => debouncer.add_event(&event),
                _ = tokio::time::sleep(debouncer.sleep_duration()) => {
                    // Ensure watcher roots remain attached.
                    watch_roots.maintain(&mut watcher);
                    // Classify recovered events and route messages.
                    if process_changes(&mut debouncer, &compiler_tx, &config).await.is_err() {
                        break;
                    }
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
    // Must be serving to process events (check BEFORE taking to preserve events)
    if !crate::core::is_serving() {
        return Ok(());
    }

    // Get raw events from debouncer (pure timing)
    let Some(raw_events) = debouncer.take_if_ready() else {
        return Ok(());
    };

    // If initial build failed, trigger retry scan on any file change
    if !crate::core::is_healthy() {
        if raw_events.is_empty() {
            return Ok(());
        }
        let changed_paths: Vec<_> = raw_events.keys().cloned().collect();
        crate::debug!("watch"; "retrying scan after change");
        compiler_tx
            .send(CompilerMsg::RetryScan { changed_paths })
            .await
            .map_err(|_| ())?;
        return Ok(());
    }

    // Classify events (business logic)
    let Some(events) = EventClassifier::classify(raw_events, config) else {
        return Ok(());
    };

    log_events(&events);

    let messages = events_to_messages(events, config);
    if messages.is_empty() {
        return Ok(());
    }

    for msg in messages {
        compiler_tx.send(msg).await.map_err(|_| ())?;
    }

    Ok(())
}
