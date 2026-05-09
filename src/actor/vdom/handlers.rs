use std::path::{Path, PathBuf};

use crate::actor::messages::WsMsg;
use crate::cache::{PersistedError, persist_diagnostics, restore_cache};
use crate::compiler::family::{CacheEntry, Indexed};
use crate::compiler::page::BUILD_CACHE;
use crate::core::UrlPath;
use crate::reload::diff::{DiffOutcome, compute_diff_shared};
use tola_vdom::prelude::*;

use super::VdomActor;
use super::permalink::PermalinkHandler;

impl VdomActor {
    fn persist_diagnostics_state(&self) {
        if let Err(e) = persist_diagnostics(&self.error_state, &self.root) {
            crate::debug!("vdom"; "diagnostics persist failed: {}", e);
        }
    }

    pub(super) async fn handle_error(&mut self, path: PathBuf, url_path: UrlPath, error: String) {
        let rel_path = self.to_relative(&path);
        let rel_path_str = rel_path.display().to_string();
        let duplicate_same_error = self
            .error_state
            .errors()
            .any(|e| e.path == rel_path_str && e.error == error);

        // Always record in batch output so watch status won't hide persistent errors.
        self.batch.push_error(&rel_path_str, &error);

        if duplicate_same_error {
            crate::debug!(
                "vdom";
                "skip duplicate compile error persist/ws: {}",
                rel_path_str
            );
        } else {
            // Track for persistence
            self.error_state.push_error(PersistedError::new(
                rel_path_str.clone(),
                url_path.to_string(),
                error.clone(),
            ));

            // Persist immediately for crash safety
            self.persist_diagnostics_state();
        }

        // Invalidate cache
        if !url_path.is_empty() {
            BUILD_CACHE.remove(&CacheKey::new(url_path.as_str()));
        }

        if !duplicate_same_error {
            let _ = self
                .ws_tx
                .send(WsMsg::Error {
                    path: rel_path_str,
                    error: crate::utils::ansi_to_html(&error),
                })
                .await;
        }
    }

    pub(super) async fn handle_process(
        &mut self,
        config: std::sync::Arc<crate::config::SiteConfig>,
        path: PathBuf,
        url_path: UrlPath,
        new_vdom: Document<Indexed>,
        permalink_change: Option<crate::address::PermalinkUpdate>,
        warnings: Vec<String>,
    ) {
        use crate::address::PermalinkUpdate;

        // Store warnings for this path
        let rel_path = self.to_relative(&path);
        let rel_path_str = rel_path.display().to_string();
        self.error_state.set_warnings(&rel_path_str, warnings);
        self.persist_diagnostics_state();

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
                        url_path: None,
                        url_change: None,
                    })
                    .await;
                return;
            }
        };

        // Handle permalink change side effects (cleanup old output file, record for logging)
        if let Some(ref old) = old_url {
            PermalinkHandler::cleanup_old_output(&config, old);
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
        let rel_path_str = rel_path.display().to_string();

        self.batch
            .push_conflict(url, rel_path.clone(), existing_rel.clone());

        let error = format!(
            "Permalink conflict: '{}' is already used by '{}'",
            url,
            existing_rel.display()
        );
        let duplicate_same_error = self
            .error_state
            .errors()
            .any(|e| e.path == rel_path_str && e.error == error);

        self.batch.push_error(rel_path_str.clone(), error.clone());

        if !duplicate_same_error {
            self.error_state.push_error(PersistedError::new(
                rel_path_str.clone(),
                url.to_string(),
                error.clone(),
            ));
            self.persist_diagnostics_state();

            let _ = self
                .ws_tx
                .send(WsMsg::Error {
                    path: rel_path_str,
                    error: crate::utils::ansi_to_html(&error),
                })
                .await;
        }
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
        if self.error_state.clear_errors_for(&rel_path_str) {
            self.persist_diagnostics_state();
            let _ = self
                .ws_tx
                .send(WsMsg::ClearError {
                    path: rel_path_str.clone(),
                })
                .await;
        }

        let priority = Some(if ACTIVE_PAGE.is_active(url_path.as_str()) {
            Priority::Active
        } else {
            Priority::Direct
        });
        let url_change = old_url.map(|old| UrlChange::new(old, url_path.clone()));

        match outcome {
            DiffOutcome::Edits(edits, new_vdom) => {
                self.handle_edits(&rel_path, url_path, edits, new_vdom, priority, url_change)
                    .await;
            }
            DiffOutcome::Initial => {
                self.handle_initial(&rel_path, url_path, priority, url_change)
                    .await;
            }
            DiffOutcome::Unchanged => {
                self.handle_unchanged(&rel_path, url_path, priority, url_change)
                    .await;
            }
            DiffOutcome::NeedsReload { reason } => {
                self.handle_needs_reload(&rel_path, url_path, reason, priority, url_change)
                    .await;
            }
        }
    }

    async fn handle_edits(
        &mut self,
        rel_path: &Path,
        url_path: UrlPath,
        edits: Vec<crate::compiler::family::DiffEdit>,
        new_vdom: Box<Document<Indexed>>,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        crate::debug_do! {
            let edit_summary: Vec<String> = edits.iter().map(|edit| edit.summary()).collect();
            crate::log!("vdom"; "reload: {} ({} edits): {:?}", rel_path.display(), edits.len(), edit_summary);
        }

        self.batch
            .push_reload(rel_path.display().to_string(), priority);

        let config = RenderConfig::default();
        let patches = render_patches(&edits, &config);

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
            BUILD_CACHE.insert(
                key,
                CacheEntry::with_default_version(tola_vdom::snapshot::project(&*new_vdom)),
            );
        }
    }

    async fn handle_initial(
        &mut self,
        rel_path: &Path,
        url_path: UrlPath,
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
                url_path: Some(url_path),
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
        url_path: UrlPath,
        reason: String,
        priority: Option<crate::core::Priority>,
        url_change: Option<crate::core::UrlChange>,
    ) {
        crate::debug!("vdom"; "reload: {}: {}", rel_path.display(), reason);
        self.batch
            .push_reload(rel_path.display().to_string(), priority);
        let _ = self
            .ws_tx
            .send(WsMsg::Reload {
                reason,
                url_path: Some(url_path),
                url_change,
            })
            .await;
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

    pub(super) async fn handle_clear_diagnostics(&mut self, path: Option<PathBuf>) {
        match path {
            Some(path) => {
                let rel_path = self.to_relative(&path);
                let rel_path_str = rel_path.display().to_string();
                let had_error = self.error_state.clear_errors_for(&rel_path_str);
                self.error_state.clear_warnings_for(&rel_path_str);
                self.persist_diagnostics_state();

                if had_error {
                    let _ = self
                        .ws_tx
                        .send(WsMsg::ClearError { path: rel_path_str })
                        .await;
                }
            }
            None => {
                let error_paths: Vec<_> = self
                    .error_state
                    .errors()
                    .map(|error| error.path.clone())
                    .collect();

                if self.error_state.is_empty() {
                    return;
                }

                self.error_state = crate::cache::PersistedDiagnostics::new();
                self.persist_diagnostics_state();

                for path in error_paths {
                    let _ = self.ws_tx.send(WsMsg::ClearError { path }).await;
                }
            }
        }
    }

    fn to_relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }
}
