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

use std::path::PathBuf;

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

/// Result of VdomActor::new() - (actor, cache_entries, restored_errors, warnings)
pub type VdomRestoreResult = (
    VdomActor,
    usize,
    Vec<crate::cache::PersistedError>,
    Vec<String>,
);

impl VdomActor {
    /// Create a new VdomActor.
    ///
    /// Attempts to restore cache and diagnostics from disk.
    /// Returns (actor, cache_count, restored_errors, warnings).
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

        let restored_errors: Vec<_> = error_state
            .errors()
            .cloned()
            .map(|error| {
                crate::cache::PersistedError::new(
                    error.path,
                    error.url_path,
                    crate::utils::ansi_to_html(&error.error),
                )
            })
            .collect();

        // Get warnings for display
        let warnings: Vec<String> = error_state.warnings().map(|w| w.warning.clone()).collect();

        // AddressSpace is rebuilt by startup `scan_pages` before serving.
        // Avoid restoring only source->url mappings from cache, which leaves url->source incomplete.

        let actor = Self {
            rx,
            ws_tx,
            root,
            batch: BatchLogger::new(),
            error_state,
        };

        (actor, restored, restored_errors, warnings)
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

                VdomMsg::ClearDiagnostics { path } => self.handle_clear_diagnostics(path).await,

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
    use crate::actor::messages::WsMsg;
    use crate::cache::restore_diagnostics;
    use crate::core::GLOBAL_ADDRESS_SPACE;
    use crate::core::UrlPath;
    use std::fs;
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

    #[tokio::test]
    async fn test_duplicate_error_keeps_batch_error_without_ws_spam() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (_tx, rx) = mpsc::channel(10);
        let (ws_tx, mut ws_rx) = mpsc::channel(10);

        let (mut actor, _, _, _) = VdomActor::new(rx, ws_tx, root.clone());
        let path = root.join("dup.typ");
        let url_path = UrlPath::from_page("/dup");
        let error = "same compile error".to_string();

        actor
            .handle_error(path.clone(), url_path.clone(), error.clone())
            .await;
        actor.handle_error(path, url_path, error).await;

        assert!(
            actor.batch.has_errors(),
            "duplicate errors should still mark batch as errored"
        );
        assert_eq!(
            actor.error_state.error_count(),
            1,
            "duplicate errors should not duplicate persisted diagnostics"
        );

        let first = ws_rx.try_recv().expect("first error should be sent to WS");
        assert!(matches!(first, WsMsg::Error { .. }));
        assert!(
            ws_rx.try_recv().is_err(),
            "duplicate identical error should not send another WS error"
        );
    }

    #[tokio::test]
    async fn test_distinct_errors_all_sent_to_ws() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (_tx, rx) = mpsc::channel(10);
        let (ws_tx, mut ws_rx) = mpsc::channel(10);

        let (mut actor, _, _, _) = VdomActor::new(rx, ws_tx, root.clone());

        actor
            .handle_error(
                root.join("articles.typ"),
                UrlPath::from_page("/articles"),
                "first error".to_string(),
            )
            .await;
        actor
            .handle_error(
                root.join("programming.typ"),
                UrlPath::from_page("/programming"),
                "second error".to_string(),
            )
            .await;

        let first = ws_rx.try_recv().expect("first error should be sent");
        let second = ws_rx.try_recv().expect("second error should be sent");

        assert!(matches!(first, WsMsg::Error { .. }));
        assert!(matches!(second, WsMsg::Error { .. }));
    }

    #[tokio::test]
    async fn test_clear_diagnostics_for_path_persists_and_notifies_ws() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (_tx, rx) = mpsc::channel(10);
        let (ws_tx, mut ws_rx) = mpsc::channel(10);

        let (mut actor, _, _, _) = VdomActor::new(rx, ws_tx, root.clone());
        actor
            .handle_error(
                root.join("clear.typ"),
                UrlPath::from_page("/clear"),
                "clear me".to_string(),
            )
            .await;

        let _ = ws_rx.try_recv().expect("initial error should be sent");

        actor
            .handle_clear_diagnostics(Some(root.join("clear.typ")))
            .await;

        let cleared = ws_rx.try_recv().expect("clear_error should be sent");
        assert!(matches!(cleared, WsMsg::ClearError { .. }));

        let state = restore_diagnostics(&root).unwrap();
        assert_eq!(
            state.error_count(),
            0,
            "path clear should persist empty errors"
        );
    }

    #[test]
    fn test_new_does_not_restore_address_space_from_cache() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let cache_dir = root.join(".tola/cache");
        fs::create_dir_all(&cache_dir).unwrap();

        // Populate cache index with one entry. VdomActor::new should not mutate AddressSpace.
        fs::write(
            cache_dir.join("index.json"),
            r#"{"entries":{"/from-cache/":{"filename":"from-cache","source_path":"content/from-cache.typ","source_hash":"abc","dependencies":{}}},"created_at":0}"#,
        )
        .unwrap();

        GLOBAL_ADDRESS_SPACE.write().clear();

        let (_tx, rx) = mpsc::channel(1);
        let (ws_tx, _ws_rx) = mpsc::channel(1);
        let _ = VdomActor::new(rx, ws_tx, root);

        assert!(
            GLOBAL_ADDRESS_SPACE.read().is_empty(),
            "AddressSpace should be rebuilt by scan_pages, not partially restored from cache"
        );
    }
}
