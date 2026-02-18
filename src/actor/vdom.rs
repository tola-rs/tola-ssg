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

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use tokio::sync::mpsc;

use super::messages::{VdomMsg, WsMsg};
use crate::cache::{
    PersistedError, PersistedErrorState, persist_cache, persist_errors, restore_cache,
    restore_dependency_graph, restore_errors,
};
use crate::compiler::family::{CacheEntry, Indexed};
use crate::compiler::page::BUILD_CACHE;
use crate::core::{GLOBAL_ADDRESS_SPACE, UrlPath};
use crate::logger::WatchStatus;
use crate::reload::diff::{DiffOutcome, compute_diff_shared};
use tola_vdom::prelude::*;

/// Batch entry status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchStatus {
    Reload,
    Unchanged,
    Error,
}

/// Entry for batch log output
#[derive(Debug, Clone)]
struct BatchEntry {
    path: String,
    status: BatchStatus,
    error: Option<String>,
    priority: Option<crate::core::Priority>,
}

impl BatchEntry {
    fn reload(path: impl Into<String>, priority: Option<crate::core::Priority>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Reload,
            error: None,
            priority,
        }
    }

    fn unchanged(path: impl Into<String>, priority: Option<crate::core::Priority>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Unchanged,
            error: None,
            priority,
        }
    }

    fn error(path: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            status: BatchStatus::Error,
            error: Some(error.into()),
            priority: None,
        }
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn is_error(&self) -> bool {
        self.status == BatchStatus::Error
    }

    fn is_unchanged(&self) -> bool {
        self.status == BatchStatus::Unchanged
    }

    fn is_reload(&self) -> bool {
        self.status == BatchStatus::Reload
    }

    fn is_primary(&self) -> bool {
        self.priority == Some(crate::core::Priority::Active)
    }

    fn error_detail(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Aggregates batch results and conflicts for unified output
struct BatchLogger {
    results: Vec<BatchEntry>,
    conflicts: FxHashMap<UrlPath, Vec<PathBuf>>,
    permalink_changes: Vec<(PathBuf, UrlPath, UrlPath)>, // (path, old_url, new_url)
    status: WatchStatus,
}

impl BatchLogger {
    fn new() -> Self {
        Self {
            results: Vec::new(),
            conflicts: FxHashMap::default(),
            permalink_changes: Vec::new(),
            status: WatchStatus::new(),
        }
    }

    /// Record a successful reload.
    fn push_reload(&mut self, path: impl Into<String>, priority: Option<crate::core::Priority>) {
        self.results.push(BatchEntry::reload(path, priority));
    }

    /// Record an unchanged result.
    fn push_unchanged(&mut self, path: impl Into<String>, priority: Option<crate::core::Priority>) {
        self.results.push(BatchEntry::unchanged(path, priority));
    }

    /// Record an error.
    fn push_error(&mut self, path: impl Into<String>, error: impl Into<String>) {
        self.results.push(BatchEntry::error(path, error));
    }

    /// Check if there are any errors in the current batch.
    fn has_errors(&self) -> bool {
        self.results.iter().any(|e| e.is_error())
    }

    /// Record a permalink conflict.
    fn push_conflict(&mut self, url: &UrlPath, source: PathBuf, existing: PathBuf) {
        let sources = self.conflicts.entry(url.clone()).or_default();
        if !sources.contains(&existing) {
            sources.push(existing);
        }
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Record a permalink change.
    fn push_permalink_change(&mut self, path: PathBuf, old_url: UrlPath, new_url: UrlPath) {
        self.permalink_changes.push((path, old_url, new_url));
    }

    /// Output all conflicts and results, then clear.
    fn flush(&mut self) {
        self.output_permalink_changes();
        self.output_conflicts();
        self.output_results();
    }

    fn output_permalink_changes(&mut self) {
        for (path, old_url, new_url) in &self.permalink_changes {
            crate::log!("permalink"; "{}: \"{}\" -> \"{}\"", path.display(), old_url, new_url);
        }
        self.permalink_changes.clear();
    }

    fn output_conflicts(&mut self) {
        for (url, sources) in &self.conflicts {
            let sources_str = sources
                .iter()
                .map(|p| format!("`{}`", p.display()))
                .collect::<Vec<_>>()
                .join(", ");
            crate::log!("conflict"; "url \"{}\" owned by {}", url, sources_str);
        }
        self.conflicts.clear();
    }

    fn output_results(&mut self) {
        if self.results.is_empty() {
            return;
        }

        let errors: Vec<_> = self.results.iter().filter(|e| e.is_error()).collect();
        let reloads: Vec<_> = self.results.iter().filter(|e| e.is_reload()).collect();
        let unchanged: Vec<_> = self.results.iter().filter(|e| e.is_unchanged()).collect();

        let primary_reload = reloads.iter().find(|e| e.is_primary()).or(reloads.first());

        if !errors.is_empty() {
            let primary_error = &errors[0];
            let detail = primary_error.error_detail().unwrap_or("");
            let summary = format!("compile error in {}", primary_error.path());
            self.status.error(&summary, detail);
        } else {
            // Show warnings only when no errors
            let warnings = crate::compiler::drain_warnings();
            if !warnings.is_empty() {
                self.status.warning(&warnings.to_string());
            }

            if let Some(primary) = primary_reload {
                let other_count = reloads.len() - 1 + unchanged.len();
                let msg = match other_count {
                    0 => format!("reload: {}", primary.path()),
                    1 => {
                        let other = reloads
                            .iter()
                            .chain(unchanged.iter())
                            .find(|e| e.path() != primary.path())
                            .map(|e| e.path())
                            .unwrap_or("?");
                        format!("reload: {}, other: {}", primary.path(), other)
                    }
                    n => format!("reload: {}, others: {}", primary.path(), n),
                };
                self.status.success(&msg);
            } else if !unchanged.is_empty() {
                let first = unchanged[0].path();
                let msg = match unchanged.len() {
                    1 => format!("unchanged: {}", first),
                    n => format!("unchanged: {}, others: {}", first, n - 1),
                };
                self.status.unchanged(&msg);
            }
        }

        self.results.clear();
    }
}

/// Handles permalink change side effects (old file cleanup)
///
/// Note: Permalink change detection is now done in CompilerActor
/// This handler only processes the side effects
struct PermalinkHandler;

impl PermalinkHandler {
    /// Cleanup old output file.
    fn cleanup_old_output(old_url: &UrlPath) {
        use crate::config::cfg;

        let config = cfg();
        let output_dir = config.paths().output_dir();
        let rel_path = old_url.as_str().trim_matches('/');
        let old_file = if rel_path.is_empty() {
            output_dir.join("index.html")
        } else {
            output_dir.join(rel_path).join("index.html")
        };

        if old_file.exists() {
            if let Err(e) = std::fs::remove_file(&old_file) {
                crate::debug!("vdom"; "failed to remove {}: {}", old_file.display(), e);
                return;
            }
            crate::debug!("vdom"; "removed old output {}", old_file.display());
        }

        // Remove empty parent directory
        if let Some(parent) = old_file.parent()
            && parent.is_dir()
            && std::fs::read_dir(parent)
                .map(|mut e| e.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

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
    error_state: PersistedErrorState,
}

/// Result of VdomActor::new() - (actor, cache_entries, first_error)
pub type VdomRestoreResult = (VdomActor, usize, Option<(String, String)>);

impl VdomActor {
    /// Create a new VdomActor.
    ///
    /// Attempts to restore cache and errors from disk.
    /// Returns (actor, cache_count, first_error) where first_error is (path, error).
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

        // Restore errors from disk
        let error_state = restore_errors(&root).unwrap_or_default();

        // Get first error for WsActor initialization
        let first_error = error_state
            .first()
            .map(|e| (e.path.clone(), crate::utils::ansi_to_html(&e.error)));

        // Restore AddressSpace from cache (skip if scan already populated it)
        if !crate::core::is_scan_completed() {
            Self::restore_address_space(&root);
        }

        let actor = Self {
            rx,
            ws_tx,
            root,
            batch: BatchLogger::new(),
            error_state,
        };

        (actor, restored, first_error)
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
                } => {
                    self.handle_process(path, url_path, *vdom, permalink_change)
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
        for error in self.error_state.iter() {
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
                url_change: None,
            })
            .await;
    }

    async fn handle_error(&mut self, path: PathBuf, url_path: UrlPath, error: String) {
        let rel_path = self.to_relative(&path);
        let rel_path_str = rel_path.display().to_string();

        // Check if this is the first error in the batch (before adding)
        let is_first_error = !self.batch.has_errors();

        // Record for batch output
        self.batch.push_error(&rel_path_str, &error);

        // Track for persistence
        self.error_state.push(PersistedError::new(
            rel_path_str.clone(),
            url_path.to_string(),
            error.clone(),
        ));

        // Persist immediately for crash safety
        if let Err(e) = persist_errors(&self.error_state, &self.root) {
            crate::debug!("vdom"; "error persist failed: {}", e);
        }

        // Invalidate cache
        if !url_path.is_empty() {
            BUILD_CACHE.remove(&CacheKey::new(url_path.as_str()));
        }

        // Send to browser only if this is the first error (matches terminal behavior)
        if is_first_error {
            let _ = self
                .ws_tx
                .send(WsMsg::Error {
                    path: rel_path_str,
                    error: crate::utils::ansi_to_html(&error),
                })
                .await;
        }
    }

    async fn handle_process(
        &mut self,
        path: PathBuf,
        url_path: UrlPath,
        new_vdom: Document<Indexed>,
        permalink_change: Option<crate::address::PermalinkUpdate>,
    ) {
        use crate::address::PermalinkUpdate;

        // Handle permalink conflict early (detected by CompilerActor)
        if let Some(PermalinkUpdate::Conflict {
            url,
            existing_source,
        }) = &permalink_change
        {
            self.handle_permalink_conflict(&path, url, existing_source)
                .await;
            return;
        }

        // Try to reload cache if empty (handles race with background build)
        self.try_reload_cache_if_empty();

        crate::debug!("vdom"; "handle_process: url={}, cache_size={}", url_path, BUILD_CACHE.len());

        // Handle permalink change BEFORE diff (rename cache key so diff can find it)
        let old_url = if let Some(PermalinkUpdate::Changed { old_url }) = &permalink_change {
            // Rename cache entry from old_url to new_url
            let old_key = CacheKey::new(old_url.as_str());
            let new_key = CacheKey::new(url_path.as_str());
            if let Some(entry) = BUILD_CACHE.remove(&old_key) {
                BUILD_CACHE.insert(new_key, entry);
            }
            Some(old_url.clone())
        } else {
            None
        };

        // Compute diff (now using new_url as key, which has the renamed cache entry)
        let key = CacheKey::new(url_path.as_str());
        let result =
            tokio::task::spawn_blocking(move || compute_diff_shared(&BUILD_CACHE, key, new_vdom))
                .await;

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                crate::log!("vdom"; "spawn_blocking error: {}", e);
                let _ = self
                    .ws_tx
                    .send(WsMsg::Reload {
                        reason: format!("internal error: {}", e),
                        url_change: None,
                    })
                    .await;
                return;
            }
        };

        // Handle permalink change side effects (cleanup old output file, record for logging)
        if let Some(ref old) = old_url {
            PermalinkHandler::cleanup_old_output(old);
            // Record permalink change for batch output
            let rel_path = self.to_relative(&path);
            self.batch
                .push_permalink_change(rel_path, old.clone(), url_path.clone());
        }

        self.route_outcome(&path, url_path, outcome, old_url).await;
    }

    async fn handle_permalink_conflict(&mut self, path: &Path, url: &UrlPath, existing: &Path) {
        let rel_path = self.to_relative(path);
        let existing_rel = self.to_relative(existing);

        self.batch
            .push_conflict(url, rel_path.clone(), existing_rel.clone());

        let error = format!(
            "Permalink conflict: '{}' is already used by '{}'",
            url,
            existing_rel.display()
        );
        let _ = self
            .ws_tx
            .send(WsMsg::Error {
                path: rel_path.display().to_string(),
                error,
            })
            .await;
    }

    async fn route_outcome(
        &mut self,
        path: &Path,
        url_path: UrlPath,
        outcome: DiffOutcome,
        old_url: Option<UrlPath>,
    ) {
        use crate::core::{Priority, UrlChange};
        use crate::reload::active::ACTIVE_PAGE;

        let rel_path = self.to_relative(path);
        let rel_path_str = rel_path.display().to_string();

        // Clear any previous errors for this file (compilation succeeded)
        if self.error_state.clear_for(&rel_path_str) {
            // Notify browser to hide error overlay
            let _ = self.ws_tx.send(WsMsg::ClearError).await;
        }

        let priority = Some(if ACTIVE_PAGE.is_active(url_path.as_str()) {
            Priority::Active
        } else {
            Priority::Direct
        });
        let url_change = old_url.map(|old| UrlChange::new(old, url_path.clone()));

        match outcome {
            DiffOutcome::Patches(ops, new_vdom) => {
                self.handle_patches(&rel_path, url_path, ops, new_vdom, priority, url_change)
                    .await;
            }
            DiffOutcome::Initial => {
                self.handle_initial(&rel_path, priority, url_change).await;
            }
            DiffOutcome::Unchanged => {
                self.handle_unchanged(&rel_path, url_path, priority, url_change)
                    .await;
            }
            DiffOutcome::NeedsReload { reason } => {
                self.handle_needs_reload(&rel_path, reason, priority, url_change)
                    .await;
            }
        }
    }

    async fn handle_patches(
        &mut self,
        rel_path: &Path,
        url_path: UrlPath,
        ops: Vec<crate::compiler::family::PatchOp>,
        new_vdom: Box<Document<Indexed>>,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        crate::debug!("vdom"; "reload: {} ({} ops): {:?}", rel_path.display(), ops.len(), ops);

        self.batch
            .push_reload(rel_path.display().to_string(), priority);

        let config = RenderConfig::default();
        let patches = render_patches(&ops, &config);

        if self
            .ws_tx
            .send(WsMsg::Patch {
                url_path: url_path.clone(),
                patches,
                url_change,
            })
            .await
            .is_ok()
        {
            let key = CacheKey::new(url_path.as_str());
            BUILD_CACHE.insert(key, CacheEntry::with_default_version(*new_vdom));
        }
    }

    async fn handle_initial(
        &mut self,
        rel_path: &Path,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        crate::debug!("vdom"; "initial {}", rel_path.display());
        self.batch
            .push_reload(rel_path.display().to_string(), priority);
        let _ = self
            .ws_tx
            .send(WsMsg::Reload {
                reason: "initial compile".to_string(),
                url_change,
            })
            .await;
    }

    async fn handle_unchanged(
        &mut self,
        rel_path: &Path,
        url_path: UrlPath,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        if let Some(change) = url_change {
            // Permalink changed but content unchanged
            // Don't push to results - permalink change is already logged separately
            let _ = self
                .ws_tx
                .send(WsMsg::Patch {
                    url_path,
                    patches: vec![],
                    url_change: Some(change),
                })
                .await;
        } else {
            // No change, no need to notify client
            self.batch
                .push_unchanged(rel_path.display().to_string(), priority);
        }
    }

    async fn handle_needs_reload(
        &mut self,
        rel_path: &Path,
        reason: String,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        crate::debug!("vdom"; "reload: {}: {}", rel_path.display(), reason);
        self.batch
            .push_reload(rel_path.display().to_string(), priority);
        let _ = self.ws_tx.send(WsMsg::Reload { reason, url_change }).await;
    }

    fn try_reload_cache_if_empty(&self) {
        if BUILD_CACHE.is_empty() {
            match restore_cache(&BUILD_CACHE, &self.root) {
                Ok(n) if n > 0 => {
                    crate::debug!("vdom"; "reloaded {} cache entries from disk", n);
                }
                Ok(_) => {}
                Err(e) => {
                    crate::debug!("vdom"; "cache reload failed: {}", e);
                }
            }
        }
    }

    fn to_relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }

    fn persist_state(&self) {
        let source_paths = GLOBAL_ADDRESS_SPACE.read().source_paths();
        match persist_cache(&BUILD_CACHE, &source_paths, &self.root) {
            Ok(n) => crate::debug!("vdom"; "persisted {} cache entries", n),
            Err(e) => crate::debug!("vdom"; "cache persist failed: {}", e),
        }
        if let Err(e) = persist_errors(&self.error_state, &self.root) {
            crate::debug!("vdom"; "error persist failed: {}", e);
        }
        crate::debug!("vdom"; "shutting down");
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::cache::restore_errors;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_error_persistence_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel(10);
        let (ws_tx, _ws_rx) = mpsc::channel(10);

        let (actor, _, _) = VdomActor::new(rx, ws_tx, root.clone());
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

        let state = restore_errors(&root).unwrap();
        assert_eq!(state.count(), 1, "Should have 1 persisted error");
        let error = state.first().unwrap();
        assert_eq!(error.error, "test error");
    }
}
