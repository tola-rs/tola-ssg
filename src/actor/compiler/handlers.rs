use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHashSet;

use super::tasks::spawn_batch;
use super::utils::{
    cleanup_removed_assets, format_asset_reason, is_reloadable_output_asset, log_asset_errors,
    process_assets,
};
use super::{ACTIVE_RECOMPILE_COOLDOWN, BackgroundTask, CompilerActor};
use crate::actor::messages::VdomMsg;
use crate::config::SiteConfig;
use crate::hooks::HookPhase;
use crate::page::CompiledPage;
use crate::reload::classify::{collect_dependents, url_to_content_path};
use crate::reload::compile::cleanup_removed_source_state;
use crate::reload::queue::CompileQueue;

impl CompilerActor {
    /// Handle compile request: run hooks first, then compile.
    pub(super) async fn on_compile(
        &mut self,
        queue: CompileQueue,
        changed_paths: Vec<PathBuf>,
    ) -> Option<BackgroundTask> {
        let start = Instant::now();
        let pages_hash = self.state.with_pages(|pages| pages.pages_hash());

        // Run pre hooks before compilation so dependent assets are up to date.
        let watched_post_paths = self.collect_watched_post_paths(&changed_paths);
        let hook_side_effects_changed =
            !changed_paths.is_empty() && self.run_watched_pre_hooks(&changed_paths);
        let atomic_css_changed = self.refresh_atomic_css(self.config.current()).await;
        let changed_set: FxHashSet<PathBuf> = changed_paths.iter().cloned().collect();

        let direct: Vec<_> = queue.direct_files().cloned().collect();
        let affected: Vec<_> = queue.affected_files().cloned().collect();
        let queued_pages: FxHashSet<PathBuf> =
            direct.iter().chain(affected.iter()).cloned().collect();
        for path in &direct {
            if self.should_skip_noop_change(
                path,
                &changed_set,
                hook_side_effects_changed || atomic_css_changed,
            ) {
                crate::debug!("compile"; "skip no-op save: {}", path.display());
                continue;
            }
            self.compile_one(path).await;
        }

        crate::debug!("compile"; "{} direct, {} affected", direct.len(), affected.len());

        if hook_side_effects_changed {
            self.compile_active_pages_except("hook", changed_paths.len(), &queued_pages, false)
                .await;
        }

        if atomic_css_changed {
            self.compile_active_pages_except(
                "atomic-css",
                changed_paths.len(),
                &queued_pages,
                false,
            )
            .await;
        }

        if affected.is_empty() {
            self.finish_batch(self.config.current(), pages_hash, watched_post_paths)
                .await;
            crate::debug!("compile"; "done in {:?}", start.elapsed());
            None
        } else {
            let (config, typst_host) = self.current_config_and_typst_host();
            Some(spawn_batch(
                affected,
                config,
                typst_host,
                Arc::clone(&self.state),
                pages_hash,
                watched_post_paths,
                self.page_epoch.ticket(),
            ))
        }
    }

    /// Run watched pre hooks and return whether hook side effects may have changed output files.
    ///
    /// When hooks execute, clear cached asset versions so the same compilation
    /// round uses fresh `?v=` links for opaque hook side effects.
    fn run_watched_pre_hooks(&self, changed_paths: &[PathBuf]) -> bool {
        use crate::hooks;

        let config = self.config.current();
        let refs = Self::path_refs(changed_paths);
        let executed = hooks::run_watched_pre_hooks(&config, &refs);
        self.invalidate_asset_versions_after_hooks(HookPhase::Pre, executed)
    }

    /// Run watched post hooks and return whether hook side effects may have changed output files.
    pub(super) fn run_watched_post_hooks(&self, changed_paths: &[PathBuf]) -> bool {
        use crate::hooks;

        let config = self.config.current();
        let refs = Self::path_refs(changed_paths);
        let executed = hooks::run_watched_post_hooks(&config, &refs);
        self.invalidate_asset_versions_after_hooks(HookPhase::Post, executed)
    }

    /// Capture changed paths for post hooks only when a watched post hook exists.
    fn collect_watched_post_paths(&self, changed_paths: &[PathBuf]) -> Option<Vec<PathBuf>> {
        use crate::hooks;

        if changed_paths.is_empty() {
            return None;
        }

        let config = self.config.current();
        let refs = Self::path_refs(changed_paths);
        hooks::has_watched_post_hooks(&config, &refs).then(|| changed_paths.to_vec())
    }

    fn path_refs(paths: &[PathBuf]) -> Vec<&Path> {
        paths.iter().map(|p| p.as_path()).collect()
    }

    fn invalidate_asset_versions_after_hooks(&self, phase: HookPhase, executed: usize) -> bool {
        use crate::asset::version;

        if executed == 0 {
            return false;
        }

        version::clear();
        crate::debug!(
            phase.as_str();
            "{} watched hooks executed: {}, cleared asset versions",
            phase.as_str(),
            executed
        );
        true
    }

    async fn refresh_atomic_css(&self, config: Arc<SiteConfig>) -> bool {
        if !config.build.atomic_css.enable {
            return false;
        }

        match tokio::task::spawn_blocking(move || crate::css::build::build(&config)).await {
            Ok(Ok(Some(output))) => output.written,
            Ok(Ok(None)) => false,
            Ok(Err(e)) => {
                crate::log!("error"; "atomic CSS: {:#}", e);
                let _ = self
                    .vdom_tx
                    .send(VdomMsg::Reload {
                        reason: format!("atomic CSS failed: {e}"),
                    })
                    .await;
                false
            }
            Err(e) => {
                crate::debug!("compile"; "atomic CSS task failed: {}", e);
                false
            }
        }
    }

    /// Skip recompilation for no-op saves.
    fn should_skip_noop_change(
        &self,
        path: &Path,
        changed_set: &FxHashSet<PathBuf>,
        hook_side_effects_changed: bool,
    ) -> bool {
        if hook_side_effects_changed || !changed_set.contains(path) {
            return false;
        }

        let config = self.config.current();
        let Ok(page) = CompiledPage::from_paths(path, &config) else {
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

        let pages_hash = self.state.with_pages(|pages| pages.pages_hash());

        let atomic_css_changed = self.refresh_atomic_css(self.config.current()).await;

        for path in &paths {
            self.compile_one(path).await;
        }

        if atomic_css_changed {
            let excluded: FxHashSet<PathBuf> = paths.iter().cloned().collect();
            self.compile_active_pages_except("atomic-css", count, &excluded, false)
                .await;
        }

        self.finish_batch(self.config.current(), pages_hash, None)
            .await;
    }

    /// Handle deleted content files and cleanup all related state.
    pub(super) async fn on_content_removed(&mut self, paths: Vec<PathBuf>) {
        let count = paths.len();
        crate::debug!("watch"; "{} content files removed", count);
        let config = self.config.current();

        for path in &paths {
            if let Some(url) = cleanup_removed_source_state(path, &config, &self.state) {
                crate::debug!("watch"; "cleaned up {} -> {}", path.display(), url);
            }

            let _ = self
                .vdom_tx
                .send(VdomMsg::ClearDiagnostics {
                    path: Some(path.clone()),
                })
                .await;
        }

        if self.refresh_atomic_css(self.config.current()).await {
            self.compile_active_pages_except("atomic-css", count, &FxHashSet::default(), false)
                .await;
        }

        self.recompile_virtual_users().await;
        self.write_seo_outputs(self.config.current()).await;
        let _ = self
            .vdom_tx
            .send(VdomMsg::BatchEnd {
                config: self.config.current(),
            })
            .await;
    }

    pub(super) async fn on_asset_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let config = self.config.current();
        let count = paths.len();
        let existing_paths: Vec<_> = paths.iter().filter(|path| path.exists()).cloned().collect();
        let removed_count = cleanup_removed_assets(&paths, &config);

        let errors = tokio::task::spawn_blocking({
            let paths = existing_paths.clone();
            let config = Arc::clone(&config);
            move || process_assets(&paths, &config)
        })
        .await
        .unwrap_or_default();

        log_asset_errors(&errors);

        let mut any_changed = false;
        for path in &existing_paths {
            if version::update_version(path) {
                any_changed = true;
            }
        }
        any_changed |= removed_count > 0;

        if any_changed {
            self.recompile_active_pages("asset", count).await;
        } else if !errors.is_empty() {
            let reason = format_asset_reason(count, errors.len());
            let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
        }
    }

    pub(super) async fn on_output_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let config = self.config.current();
        let total = paths.len();
        let mut unique = FxHashSet::default();
        let output_assets: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| is_reloadable_output_asset(path, &config))
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
        if !self
            .compile_active_pages_except(tag, changed_count, &FxHashSet::default(), true)
            .await
        {
            return;
        }

        let _ = self
            .vdom_tx
            .send(VdomMsg::BatchEnd {
                config: self.config.current(),
            })
            .await;
    }

    async fn compile_active_pages_except(
        &mut self,
        tag: &str,
        changed_count: usize,
        excluded: &FxHashSet<PathBuf>,
        throttle: bool,
    ) -> bool {
        use crate::reload::active::ACTIVE_PAGE;

        if throttle && self.should_throttle_active_recompile() {
            crate::debug!(
                tag;
                "throttled active-page recompile for {} changed files",
                changed_count
            );
            return false;
        }

        let active_urls = ACTIVE_PAGE.get_all();
        if active_urls.is_empty() {
            return false;
        }

        let active_paths: Vec<_> = active_urls
            .iter()
            .filter_map(|url| url_to_content_path(url.as_str(), &self.state))
            .filter(|path| !excluded.contains(path))
            .collect();

        if active_paths.is_empty() {
            return false;
        }

        crate::log!(
            tag;
            "{} files changed, recompiling {} active pages",
            changed_count,
            active_paths.len()
        );

        for path in active_paths {
            self.compile_one(&path).await;
        }
        self.last_active_recompile = Some(Instant::now());
        true
    }

    fn should_throttle_active_recompile(&self) -> bool {
        self.last_active_recompile
            .is_some_and(|last| last.elapsed() < ACTIVE_RECOMPILE_COOLDOWN)
    }

    pub(super) async fn on_full_rebuild(&mut self) {
        use crate::asset::version;
        use crate::compiler::dependency::clear_graph;
        use crate::compiler::scheduler::SCHEDULER;
        use crate::core::{BuildMode, set_healthy};
        use crate::reload::active::ACTIVE_PAGE;

        crate::debug!("compile"; "full rebuild triggered");
        set_healthy(false);

        let _ = self.config.reload();

        clear_graph();
        version::clear();
        SCHEDULER.clear_cache();
        let _ = self.vdom_tx.send(VdomMsg::Clear).await;

        let config = self.config.current();
        let state = Arc::clone(&self.state);
        let result = tokio::task::spawn_blocking(move || {
            crate::cli::build::build_site(BuildMode::DEVELOPMENT, &config, &state, true)
        })
        .await;

        match result {
            Ok(Ok(_)) => {
                set_healthy(true);
                self.config.clear_clean_flag();
                crate::debug!("compile"; "full rebuild complete");

                let _ = self
                    .vdom_tx
                    .send(VdomMsg::ClearDiagnostics { path: None })
                    .await;

                let active_urls = ACTIVE_PAGE.get_all();
                if !active_urls.is_empty() {
                    crate::debug!(
                        "compile";
                        "recompiling {} active pages after rebuild",
                        active_urls.len()
                    );
                    for url in active_urls {
                        if let Some(path) = url_to_content_path(url.as_str(), &self.state) {
                            self.compile_one(&path).await;
                        }
                    }
                }
                self.write_seo_outputs(self.config.current()).await;
            }
            Ok(Err(e)) => {
                crate::debug!("compile"; "full rebuild failed: {}", e);
                let reason = format!("rebuild failed: {}", e);
                let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
            }
            Err(e) => {
                crate::debug!("compile"; "spawn_blocking error: {}", e);
                let reason = format!("internal error: {}", e);
                let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
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

        let (config, typst_host) = self.current_config_and_typst_host();
        let scan_config = Arc::clone(&config);
        let scan_host = Arc::clone(&typst_host);
        let state = Arc::clone(&self.state);
        let result =
            tokio::task::spawn_blocking(move || scan_pages(&scan_config, &scan_host, &state)).await;

        match result {
            Ok(Ok(_)) => {
                crate::debug!("scan"; "recovered");

                self.refresh_atomic_css(Arc::clone(&config)).await;

                let content_files: Vec<_> = changed_paths
                    .iter()
                    .filter(|p| matches!(categorize_path(p, &config), FileCategory::Content(_)))
                    .cloned()
                    .collect();

                for path in &content_files {
                    self.compile_one(path).await;
                }

                let active_urls = ACTIVE_PAGE.get_all();
                for url in active_urls {
                    if let Some(path) = url_to_content_path(url.as_str(), &self.state)
                        && !content_files.contains(&path)
                    {
                        self.compile_one(&path).await;
                    }
                }

                set_healthy(true);
                let _ = self
                    .vdom_tx
                    .send(VdomMsg::BatchEnd {
                        config: self.config.current(),
                    })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::*;
    use crate::actor::messages::{CompilerMsg, VdomMsg};
    use crate::address::SiteIndex;
    use crate::config::{SiteConfig, config_handle, init_config};
    use crate::core::UrlPath;
    use crate::page::PageRoute;
    use crate::reload::active::ACTIVE_PAGE;

    struct ResetGlobals;

    impl Drop for ResetGlobals {
        fn drop(&mut self) {
            ACTIVE_PAGE.clear();
            crate::asset::version::clear();
            init_config(SiteConfig::default());
        }
    }

    #[tokio::test]
    async fn atomic_css_only_change_recompiles_active_pages() {
        let _reset = ResetGlobals;
        crate::asset::version::clear();
        ACTIVE_PAGE.clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let components = root.join("components");
        let output = root.join("public");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&components).unwrap();

        let page = content.join("index.md");
        let component = components.join("button.html");
        std::fs::write(&page, "+++\ntitle = \"Home\"\n+++\n\n# Home\n").unwrap();
        std::fs::write(&component, r#"<button class="flex"></button>"#).unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output.clone();
        config.build.atomic_css.enable = true;
        init_config(config);

        let state = Arc::new(SiteIndex::new());
        state.edit(|_, address| {
            address.register_page(
                PageRoute {
                    source: page.clone(),
                    permalink: UrlPath::from_page("/"),
                    output_file: output.join("index.html"),
                    output_dir: output.clone(),
                    is_index: true,
                    is_404: false,
                    full_url: String::new(),
                },
                Some("Home".into()),
            );
        });

        ACTIVE_PAGE.add(UrlPath::from_page("/"));

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(compiler_rx, vdom_tx, config_handle(), state);

        let background = actor
            .on_compile(CompileQueue::new(), vec![component.clone()])
            .await;

        assert!(background.is_none());

        let mut saw_active_page = false;
        while let Ok(message) = vdom_rx.try_recv() {
            if let VdomMsg::Process { path, .. } = message
                && path == page
            {
                saw_active_page = true;
            }
        }

        assert!(
            saw_active_page,
            "Atomic CSS-only changes must refresh active page stylesheet links"
        );
    }
}
