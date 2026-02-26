//! VDOM Actor - The Bridge between Typst and Hot Reload
//!
//! This actor is responsible for:
//! - Receiving compiled VDOM from CompilerActor
//! - Computing diffs via `pipeline::diff`
//! - Managing VDOM cache via `pipeline::diff`
//! - Sending patch/reload messages to WsActor
//! - Persisting cache for fast restarts
//!
//! # Architecture
//!
//! The actor is organized into helper structures:
//! - `BatchLogger` - Aggregates and outputs batch results
//! - `PermalinkHandler` - Handles permalink change side effects
//! - `OutcomeRouter` - Routes diff outcomes to WsActor
//!
//! Note: Permalink change detection is done in CompilerActor.
//! This separation keeps the main actor loop thin and focused on message dispatch.

mod batch;
mod handlers;
mod permalink;

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use batch::BatchLogger;

use super::messages::{VdomMsg, WsMsg};
use crate::cache::{
    PersistedDiagnostics, persist_cache, persist_diagnostics, restore_cache,
    restore_dependency_graph, restore_diagnostics,
};
use crate::compiler::page::BUILD_CACHE;
use crate::core::GLOBAL_ADDRESS_SPACE;

/// VDOM Actor - converts AST to VDOM and computes diffs
///
/// This is a thin wrapper that delegates to helper structures:
/// - `BatchLogger` for aggregated output
/// - `PermalinkHandler` for permalink changes
///
/// Uses the global `BUILD_CACHE` for VDOM storage, shared with the scheduler
/// This ensures on-demand compiled pages are available for hot reload diffing
pub struct VdomActor {
    rx: mpsc::Receiver<VdomMsg>,
    ws_tx: mpsc::Sender<WsMsg>,
    root: PathBuf,
    batch: BatchLogger,
    error_state: PersistedDiagnostics,
}

/// Result of VdomActor::new() - (actor, cache_entries, first_error, warnings)
pub type VdomRestoreResult = (VdomActor, usize, Option<(String, String)>, Vec<String>);

impl VdomActor {
    /// Create a new VdomActor.
    ///
    /// Attempts to restore cache and diagnostics from disk.
    /// Returns (actor, cache_count, first_error, warnings).
    pub fn new(
        rx: mpsc::Receiver<VdomMsg>,
        ws_tx: mpsc::Sender<WsMsg>,
        root: PathBuf,
    ) -> VdomRestoreResult {
        // Restore cache from disk into BUILD_CACHE (shared with scheduler)
        let restored = restore_cache(&BUILD_CACHE, &root).unwrap_or_else(|e| {
            crate::debug!("vdom"; "cache restore failed: {}", e);
            0
        });

        // Restore dependency graph for incremental rebuilds
        if let Err(e) = restore_dependency_graph(&root) {
            crate::debug!("vdom"; "dependency graph restore failed: {}", e);
        }

        // Restore diagnostics from disk
        let error_state = restore_diagnostics(&root).unwrap_or_default();

        // Get first error for WsActor initialization
        let first_error = error_state
            .first_error()
            .map(|e| (e.path.clone(), crate::utils::ansi_to_html(&e.error)));

        // Get warnings for display
        let warnings: Vec<String> = error_state.warnings().map(|w| w.warning.clone()).collect();

        // Restore AddressSpace from cache (skip if scan already populated it)
        if !crate::core::is_serving() {
            Self::restore_address_space(&root);
        }

        let actor = Self {
            rx,
            ws_tx,
            root,
            batch: BatchLogger::new(),
            error_state,
        };

        (actor, restored, first_error, warnings)
    }

    fn restore_address_space(root: &Path) {
        let source_paths = crate::cache::get_source_paths(root);
        let count = source_paths.len();

        let mut space = GLOBAL_ADDRESS_SPACE.write();
        for (url, source) in source_paths {
            space.set_source_url(source, url);
        }
        crate::debug!("address_space"; "restored {} source mappings from cache", count);
    }

    /// Run the actor event loop.
    pub async fn run(mut self) {
        self.replay_errors().await;

        while let Some(msg) = self.rx.recv().await {
            match msg {
                VdomMsg::Process {
                    path,
                    url_path,
                    vdom,
                    permalink_change,
                    warnings,
                } => {
                    self.handle_process(path, url_path, *vdom, permalink_change, warnings)
                        .await
                }

                VdomMsg::Reload { reason } => self.forward_reload(reason).await,

                VdomMsg::Error {
                    path,
                    url_path,
                    error,
                } => self.handle_error(path, url_path, error).await,

                VdomMsg::Skip => {}

                VdomMsg::BatchEnd => self.batch.flush(),

                VdomMsg::Clear => {
                    crate::compiler::page::BUILD_CACHE.clear();
                    crate::debug!("vdom"; "cleared all cache");
                }

                VdomMsg::Shutdown => {
                    crate::debug!("vdom"; "shutdown requested");
                    break;
                }
            }
        }

        self.persist_state();
    }

    async fn replay_errors(&self) {
        for error in self.error_state.errors() {
            let _ = self
                .ws_tx
                .send(WsMsg::Error {
                    path: error.path.clone(),
                    error: crate::utils::ansi_to_html(&error.error),
                })
                .await;
        }
    }

    async fn forward_reload(&self, reason: String) {
        let _ = self
            .ws_tx
            .send(WsMsg::Reload {
                reason,
                url_path: None,
                url_change: None,
            })
            .await;
    }

    fn persist_state(&self) {
        let source_paths = GLOBAL_ADDRESS_SPACE.read().source_paths();
        match persist_cache(&BUILD_CACHE, &source_paths, &self.root) {
            Ok(n) => crate::debug!("vdom"; "persisted {} cache entries", n),
            Err(e) => crate::debug!("vdom"; "cache persist failed: {}", e),
        }
        // Skip if empty: initial build warnings are saved by finalize_serve_build(),
        // which runs in parallel. Persisting empty state here would overwrite them.
        if !self.error_state.is_empty()
            && let Err(e) = persist_diagnostics(&self.error_state, &self.root)
        {
            crate::debug!("vdom"; "diagnostics persist failed: {}", e);
        }
        crate::debug!("vdom"; "shutting down");
    }
}

#[cfg(test)]
mod persistence_tests {
    use crate::cache::restore_diagnostics;
    use crate::core::UrlPath;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::{VdomActor, VdomMsg};

    #[tokio::test]
    async fn test_error_persistence_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel(10);
        let (ws_tx, _ws_rx) = mpsc::channel(10);

        let (actor, _, _, _) = VdomActor::new(rx, ws_tx, root.clone());
        let actor_handle = tokio::spawn(actor.run());

        tx.send(VdomMsg::Error {
            path: root.join("test.typ"),
            url_path: UrlPath::from_page("/test"),
            error: "test error".to_string(),
        })
        .await
        .unwrap();

        tx.send(VdomMsg::Shutdown).await.unwrap();
        actor_handle.await.unwrap();

        let state = restore_diagnostics(&root).unwrap();
        assert_eq!(state.error_count(), 1, "Should have 1 persisted error");
        let error = state.first_error().unwrap();
        assert_eq!(error.error, "test error");
    }
}
