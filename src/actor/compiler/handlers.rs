use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHashSet;

use super::tasks::spawn_batch;
use super::utils::{
    format_asset_reason, is_reloadable_output_asset, log_asset_errors, process_assets,
};
use super::{ACTIVE_RECOMPILE_COOLDOWN, BackgroundTask, CompilerActor};
use crate::address::GLOBAL_ADDRESS_SPACE;
use crate::page::STORED_PAGES;
use crate::reload::classify::{collect_dependents, url_to_content_path};

impl CompilerActor {
    /// Handle compile request: run hooks first, then compile.
    pub(super) async fn on_compile(
        &mut self,
        queue: crate::reload::queue::CompileQueue,
        changed_paths: Vec<PathBuf>,
    ) -> Option<BackgroundTask> {
        let start = Instant::now();
        let pages_hash = STORED_PAGES.pages_hash();

        // Run pre hooks before compilation so dependent assets are up to date.
        let hook_outputs_changed = !changed_paths.is_empty() && self.run_pre_hooks(&changed_paths);
        let changed_set: FxHashSet<PathBuf> = changed_paths.iter().cloned().collect();

        let direct: Vec<_> = queue.direct_files().cloned().collect();
        for path in &direct {
            if self.should_skip_noop_change(path, &changed_set, hook_outputs_changed) {
                crate::debug!("compile"; "skip no-op save: {}", path.display());
                continue;
            }
            self.compile_one(path).await;
        }

        let affected: Vec<_> = queue.affected_files().cloned().collect();
        crate::debug!("compile"; "{} direct, {} affected", direct.len(), affected.len());

        if affected.is_empty() {
            self.finish_batch(pages_hash).await;
            crate::debug!("compile"; "done in {:?}", start.elapsed());
            None
        } else {
            Some(spawn_batch(affected, Arc::clone(&self.config), pages_hash))
        }
    }

    /// Run watched hooks and return whether hook outputs may have changed.
    ///
    /// When hooks execute, invalidate cached versions for output artifacts so
    /// the same compilation round uses fresh `?v=` links.
    fn run_pre_hooks(&self, changed_paths: &[PathBuf]) -> bool {
        use crate::asset::version;
        use crate::hooks;

        let refs: Vec<&Path> = changed_paths.iter().map(|p| p.as_path()).collect();
        let executed = hooks::run_watched_hooks(&self.config, &refs);
        if executed == 0 {
            return false;
        }

        let removed = version::invalidate_under(self.config.paths().output_dir().as_path());
        crate::debug!(
            "hook";
            "watched hooks executed: {}, invalidated output versions: {}",
            executed,
            removed
        );
        true
    }

    /// Skip recompilation for no-op saves.
    fn should_skip_noop_change(
        &self,
        path: &Path,
        changed_set: &FxHashSet<PathBuf>,
        hook_outputs_changed: bool,
    ) -> bool {
        if hook_outputs_changed || !changed_set.contains(path) {
            return false;
        }

        let Ok(page) = crate::page::CompiledPage::from_paths(path, &self.config) else {
            return false;
        };

        crate::freshness::is_fresh(path, &page.route.output_file, None)
    }

    pub(super) async fn on_compile_dependents(&mut self, deps: Vec<PathBuf>) {
        let affected = collect_dependents(&deps);
        if affected.is_empty() {
            crate::log!("compile"; "no dependents for {} deps", deps.len());
        } else {
            self.compile_batch_blocking(affected).await;
        }
    }

    /// Handle new content files and register them.
    pub(super) async fn on_content_created(&mut self, paths: Vec<PathBuf>) {
        let count = paths.len();
        crate::debug!("watch"; "{} new content files", count);

        let pages_hash = STORED_PAGES.pages_hash();

        for path in &paths {
            self.compile_one(path).await;
        }

        self.finish_batch(pages_hash).await;
    }

    /// Handle deleted content files and cleanup all related state.
    pub(super) async fn on_content_removed(&mut self, paths: Vec<PathBuf>) {
        use crate::compiler::dependency::remove_content;
        use crate::compiler::page::BUILD_CACHE;
        use tola_vdom::CacheKey;

        let count = paths.len();
        crate::debug!("watch"; "{} content files removed", count);

        for path in &paths {
            let url = GLOBAL_ADDRESS_SPACE.read().url_for_source(path).cloned();

            GLOBAL_ADDRESS_SPACE.write().remove_by_source(path);
            STORED_PAGES.remove_by_source(path);
            remove_content(path);

            if let Some(url) = &url {
                BUILD_CACHE.remove(&CacheKey::new(url.as_str()));
                self.clean_output_file(url);
                crate::debug!("watch"; "cleaned up {} -> {}", path.display(), url);
            }
        }

        self.recompile_virtual_users().await;
        let _ = self
            .vdom_tx
            .send(crate::actor::messages::VdomMsg::BatchEnd)
            .await;
    }

    /// Remove output HTML file and empty parent directory for a removed page.
    fn clean_output_file(&self, url: &crate::core::UrlPath) {
        let output_dir = self.config.paths().output_dir();
        let rel_path = url.as_str().trim_matches('/');
        let output_file = if rel_path.is_empty() {
            output_dir.join("index.html")
        } else {
            output_dir.join(rel_path).join("index.html")
        };

        if !output_file.exists() {
            return;
        }

        if let Err(e) = std::fs::remove_file(&output_file) {
            crate::debug!("watch"; "failed to remove {}: {}", output_file.display(), e);
            return;
        }
        crate::debug!("watch"; "removed output {}", output_file.display());

        if let Some(parent) = output_file.parent()
            && parent != output_dir
            && parent.is_dir()
            && std::fs::read_dir(parent)
                .map(|mut e| e.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
    }

    pub(super) async fn on_asset_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let config = Arc::clone(&self.config);
        let count = paths.len();

        let errors = tokio::task::spawn_blocking({
            let paths = paths.clone();
            move || process_assets(&paths, &config)
        })
        .await
        .unwrap_or_default();

        log_asset_errors(&errors);

        let mut any_changed = false;
        for path in &paths {
            if version::update_version(path) {
                any_changed = true;
            }
        }

        if any_changed {
            self.recompile_active_pages("asset", count).await;
        } else if !errors.is_empty() {
            let reason = format_asset_reason(count, errors.len());
            let _ = self
                .vdom_tx
                .send(crate::actor::messages::VdomMsg::Reload { reason })
                .await;
        }
    }

    pub(super) async fn on_output_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let total = paths.len();
        let mut unique = FxHashSet::default();
        let output_assets: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| is_reloadable_output_asset(path))
            .filter(|path| unique.insert(path.clone()))
            .collect();

        let filtered = total.saturating_sub(output_assets.len());
        if total > 0 {
            crate::debug!(
                "output";
                "events: total={}, tracked={}, filtered={}",
                total,
                output_assets.len(),
                filtered
            );
        }

        if output_assets.is_empty() {
            return;
        }

        let mut any_changed = false;
        let mut removed_count = 0usize;
        for path in &output_assets {
            if path.exists() {
                if version::update_version(path) {
                    any_changed = true;
                }
            } else {
                let _ = version::remove_version(path);
                removed_count += 1;
                any_changed = true;
            }
        }

        if removed_count > 0 {
            crate::debug!("output"; "removed tracked outputs: {}", removed_count);
        }

        if any_changed {
            self.recompile_active_pages("output", output_assets.len())
                .await;
        }
    }

    async fn recompile_active_pages(&mut self, tag: &str, changed_count: usize) {
        use crate::reload::active::ACTIVE_PAGE;

        if self.should_throttle_active_recompile() {
            crate::debug!(
                tag;
                "throttled active-page recompile for {} changed files",
                changed_count
            );
            return;
        }

        let active_urls = ACTIVE_PAGE.get_all();
        if active_urls.is_empty() {
            return;
        }

        crate::log!(
            tag;
            "{} files changed, recompiling {} active pages",
            changed_count,
            active_urls.len()
        );

        for url in active_urls {
            if let Some(path) = url_to_content_path(url.as_str(), &self.config) {
                self.compile_one(&path).await;
            }
        }
        self.last_active_recompile = Some(Instant::now());

        let _ = self
            .vdom_tx
            .send(crate::actor::messages::VdomMsg::BatchEnd)
            .await;
    }

    fn should_throttle_active_recompile(&self) -> bool {
        self.last_active_recompile
            .is_some_and(|last| last.elapsed() < ACTIVE_RECOMPILE_COOLDOWN)
    }

    pub(super) async fn on_full_rebuild(&mut self) {
        use crate::asset::version;
        use crate::compiler::dependency::clear_graph;
        use crate::config::{clear_clean_flag, reload_config};
        use crate::core::{BuildMode, set_healthy};
        use crate::reload::active::ACTIVE_PAGE;

        crate::debug!("compile"; "full rebuild triggered");

        if let Ok(true) = reload_config() {
            self.config = crate::config::cfg();
        }

        clear_graph();
        version::clear();
        let _ = self
            .vdom_tx
            .send(crate::actor::messages::VdomMsg::Clear)
            .await;

        let config = Arc::clone(&self.config);
        let result = tokio::task::spawn_blocking(move || {
            crate::cli::build::build_site(BuildMode::DEVELOPMENT, &config, true)
        })
        .await;

        match result {
            Ok(Ok(_)) => {
                set_healthy(true);
                clear_clean_flag();
                crate::debug!("compile"; "full rebuild complete");

                let active_urls = ACTIVE_PAGE.get_all();
                if !active_urls.is_empty() {
                    crate::debug!(
                        "compile";
                        "recompiling {} active pages after rebuild",
                        active_urls.len()
                    );
                    for url in active_urls {
                        if let Some(path) = url_to_content_path(url.as_str(), &self.config) {
                            self.compile_one(&path).await;
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                crate::debug!("compile"; "full rebuild failed: {}", e);
                let reason = format!("rebuild failed: {}", e);
                let _ = self
                    .vdom_tx
                    .send(crate::actor::messages::VdomMsg::Reload { reason })
                    .await;
            }
            Err(e) => {
                crate::debug!("compile"; "spawn_blocking error: {}", e);
                let reason = format!("internal error: {}", e);
                let _ = self
                    .vdom_tx
                    .send(crate::actor::messages::VdomMsg::Reload { reason })
                    .await;
            }
        }
    }

    /// Retry scan after initial failure, then compile changed files.
    pub(super) async fn on_retry_scan(&mut self, changed_paths: Vec<PathBuf>) {
        use crate::cli::serve::scan_pages;
        use crate::core::set_healthy;
        use crate::reload::active::ACTIVE_PAGE;
        use crate::reload::classify::{FileCategory, categorize_path};

        crate::debug!("compile"; "retry scan triggered");

        let config = Arc::clone(&self.config);
        let result = tokio::task::spawn_blocking(move || scan_pages(&config)).await;

        match result {
            Ok(Ok(_)) => {
                crate::debug!("scan"; "recovered");

                let content_files: Vec<_> = changed_paths
                    .iter()
                    .filter(|p| {
                        matches!(categorize_path(p, &self.config), FileCategory::Content(_))
                    })
                    .cloned()
                    .collect();

                for path in &content_files {
                    self.compile_one(path).await;
                }

                let active_urls = ACTIVE_PAGE.get_all();
                for url in active_urls {
                    if let Some(path) = url_to_content_path(url.as_str(), &self.config)
                        && !content_files.contains(&path)
                    {
                        self.compile_one(&path).await;
                    }
                }

                set_healthy(true);
                let _ = self
                    .vdom_tx
                    .send(crate::actor::messages::VdomMsg::BatchEnd)
                    .await;
            }
            Ok(Err(e)) => {
                crate::debug!("scan"; "still failing: {}", e);
            }
            Err(e) => {
                crate::debug!("compile"; "spawn_blocking error: {}", e);
            }
        }
    }
}
