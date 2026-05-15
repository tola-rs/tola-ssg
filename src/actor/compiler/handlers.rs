use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHashSet;

use super::tasks::spawn_batch;
use super::utils::{
    cleanup_removed_assets, format_asset_reason, log_asset_errors, output_path_for_asset,
    process_assets, process_configured_assets,
};
use super::{ACTIVE_RECOMPILE_COOLDOWN, BackgroundTask, CompilerActor};
use crate::actor::messages::VdomMsg;
use crate::config::SiteConfig;
use crate::hooks::HookPhase;
use crate::page::CompiledPage;
use crate::reload::classify::{collect_dependents, url_to_content_path};
use crate::reload::compile::{CompileOutcome, cleanup_removed_source_state};
use crate::reload::output;
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
        let watched_pre_paths = self.collect_watched_pre_paths(&changed_paths);
        let outputs_before = self.snapshot_internal_outputs(watched_pre_paths.is_some());
        let hook_side_effects_changed = watched_pre_paths
            .as_deref()
            .is_some_and(|paths| self.run_watched_pre_hooks(paths));
        if hook_side_effects_changed {
            self.process_configured_assets_after_pre_hooks().await;
        }
        let atomic_css_changed = self.refresh_atomic_css(self.config.current()).await;
        let output_update = if hook_side_effects_changed || atomic_css_changed {
            self.stage_internal_outputs(outputs_before.as_ref())
        } else {
            output::Update::default()
        };
        let changed_set: FxHashSet<PathBuf> = changed_paths.iter().cloned().collect();

        let direct: Vec<_> = queue.direct_files().cloned().collect();
        let affected: Vec<_> = queue.affected_files().cloned().collect();
        let queued_pages: FxHashSet<PathBuf> =
            direct.iter().chain(affected.iter()).cloned().collect();
        let defer_delivery = watched_post_paths.is_some() || !output_update.is_empty();
        let mut direct_outcomes = Vec::new();
        for path in &direct {
            crate::freshness::invalidate_cached_hash(path);
        }
        for path in &direct {
            if self.should_skip_noop_change(
                path,
                &changed_set,
                hook_side_effects_changed || atomic_css_changed,
            ) {
                crate::debug!("compile"; "skip no-op save: {}", path.display());
                continue;
            }
            if defer_delivery {
                let (outcome, _) = self.compile_one_outcome(path).await;
                direct_outcomes.push(outcome);
            } else {
                self.compile_one(path).await;
            }
        }

        crate::debug!("compile"; "{} direct, {} affected", direct.len(), affected.len());

        if hook_side_effects_changed {
            if defer_delivery {
                direct_outcomes.extend(
                    self.compile_active_page_outcomes_except(
                        "hook",
                        changed_paths.len(),
                        &queued_pages,
                        false,
                    )
                    .await,
                );
            } else {
                self.compile_active_pages_except("hook", changed_paths.len(), &queued_pages, false)
                    .await;
            }
        }

        if atomic_css_changed {
            if defer_delivery {
                direct_outcomes.extend(
                    self.compile_active_page_outcomes_except(
                        "atomic-css",
                        changed_paths.len(),
                        &queued_pages,
                        false,
                    )
                    .await,
                );
            } else {
                self.compile_active_pages_except(
                    "atomic-css",
                    changed_paths.len(),
                    &queued_pages,
                    false,
                )
                .await;
            }
        }

        if affected.is_empty() {
            self.finish_batch(
                self.config.current(),
                pages_hash,
                watched_post_paths,
                output_update,
                direct_outcomes,
            )
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
                direct_outcomes,
                output_update,
                self.page_epoch.ticket(),
            ))
        }
    }

    /// Run watched pre hooks and return whether hook side effects may affect this build.
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

    /// Run watched post hooks and return how many hooks executed.
    pub(super) fn run_watched_post_hooks(&self, changed_paths: &[PathBuf]) -> usize {
        use crate::hooks;

        let config = self.config.current();
        let refs = Self::path_refs(changed_paths);
        hooks::run_watched_post_hooks(&config, &refs)
    }

    /// Capture changed paths for pre hooks only when a watched pre hook exists.
    fn collect_watched_pre_paths(&self, changed_paths: &[PathBuf]) -> Option<Vec<PathBuf>> {
        use crate::hooks;

        if changed_paths.is_empty() {
            return None;
        }

        let config = self.config.current();
        let refs = Self::path_refs(changed_paths);
        hooks::has_watched_pre_hooks(&config, &refs).then(|| changed_paths.to_vec())
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

    async fn refresh_atomic_css_output(
        &mut self,
        config: Arc<SiteConfig>,
    ) -> (bool, output::Update) {
        if !config.build.atomic_css.enable {
            return (false, output::Update::default());
        }

        let before = output::snapshot(&config);
        let changed = self.refresh_atomic_css(config).await;
        let update = changed
            .then(|| self.stage_internal_outputs(Some(&before)))
            .unwrap_or_default();
        (changed, update)
    }

    async fn process_configured_assets_after_pre_hooks(&self) {
        let config = self.config.current();
        let errors = tokio::task::spawn_blocking(move || process_configured_assets(&config))
            .await
            .unwrap_or_default();
        log_asset_errors(&errors);
    }

    fn snapshot_internal_outputs(&self, watched_pre_hook_exists: bool) -> Option<output::Snapshot> {
        let config = self.config.current();
        (watched_pre_hook_exists || config.build.atomic_css.enable)
            .then(|| output::snapshot(&config))
    }

    fn stage_internal_outputs(&mut self, before: Option<&output::Snapshot>) -> output::Update {
        let Some(before) = before else {
            return output::Update::default();
        };

        let config = self.config.current();
        let changed = output::changed(before, &config);
        if !changed.is_empty() {
            self.stage_output_change(changed)
        } else {
            output::Update::default()
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

        let (atomic_css_changed, output_update) =
            self.refresh_atomic_css_output(self.config.current()).await;

        for path in &paths {
            self.compile_one(path).await;
        }

        if atomic_css_changed {
            let excluded: FxHashSet<PathBuf> = paths.iter().cloned().collect();
            self.compile_active_pages_except("atomic-css", count, &excluded, false)
                .await;
        }
        self.finish_batch(
            self.config.current(),
            pages_hash,
            None,
            output_update,
            Vec::new(),
        )
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

        let (atomic_css_changed, output_update) =
            self.refresh_atomic_css_output(self.config.current()).await;
        if atomic_css_changed {
            self.compile_active_pages_except("atomic-css", count, &FxHashSet::default(), false)
                .await;
        }

        self.recompile_virtual_users().await;
        self.send_output_update(output_update).await;
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
            let version_path = output_path_for_asset(path, &config).unwrap_or_else(|| path.clone());
            if version::update_version(&version_path) {
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
        let update = self.stage_output_change(paths);
        self.send_output_update(update).await;
    }

    pub(super) fn stage_output_change(&mut self, paths: Vec<PathBuf>) -> output::Update {
        use crate::asset::version;

        let config = self.config.current();
        let total = paths.len();
        let paths = self.output_echoes.filter(paths);
        let echo_count = total.saturating_sub(paths.len());
        let mut unique = FxHashSet::default();
        let output_assets: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| output::is_reloadable(path, &config))
            .filter(|path| unique.insert(path.clone()))
            .collect();

        let filtered = total.saturating_sub(echo_count + output_assets.len());
        if total > 0 {
            crate::debug!(
                "output";
                "events: total={}, tracked={}, echoes={}, filtered={}",
                total,
                output_assets.len(),
                echo_count,
                filtered
            );
        }

        if output_assets.is_empty() {
            return output::Update::default();
        }

        let mut update = output::Update::default();
        let mut needs_reload = false;
        let mut removed_count = 0usize;
        for path in &output_assets {
            if path.exists() {
                if version::update_version(path) {
                    match output::versioned_href(path, &config) {
                        Some(href) => update.add_href(href),
                        None => needs_reload = true,
                    }
                }
            } else {
                let _ = version::remove_version(path);
                removed_count += 1;
                needs_reload = true;
            }
        }

        if removed_count > 0 {
            crate::debug!("output"; "removed tracked outputs: {}", removed_count);
        }

        self.output_echoes.record(&output_assets);

        if needs_reload {
            update.require_reload(output_assets.len());
        }

        update
    }

    pub(super) async fn send_output_update(&self, update: output::Update) {
        if update.is_empty() {
            return;
        }

        if update.reload_count() > 0 {
            let reason = format!("{} output assets updated", update.reload_count());
            let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
            return;
        }

        for href in update.hrefs() {
            let _ = self
                .vdom_tx
                .send(VdomMsg::Asset { href: href.clone() })
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
        let outcomes = self
            .compile_active_page_outcomes_except(tag, changed_count, excluded, throttle)
            .await;
        let changed = !outcomes.is_empty();
        self.route_all(outcomes, self.config.current(), &[]).await;
        changed
    }

    async fn compile_active_page_outcomes_except(
        &mut self,
        tag: &str,
        changed_count: usize,
        excluded: &FxHashSet<PathBuf>,
        throttle: bool,
    ) -> Vec<CompileOutcome> {
        use crate::reload::active::ACTIVE_PAGE;

        if throttle && self.should_throttle_active_recompile() {
            crate::debug!(
                tag;
                "throttled active-page recompile for {} changed files",
                changed_count
            );
            return Vec::new();
        }

        let active_urls = ACTIVE_PAGE.get_all();
        if active_urls.is_empty() {
            return Vec::new();
        }

        let active_paths: Vec<_> = active_urls
            .iter()
            .filter_map(|url| url_to_content_path(url.as_str(), &self.state))
            .filter(|path| !excluded.contains(path))
            .collect();

        if active_paths.is_empty() {
            return Vec::new();
        }

        crate::log!(
            tag;
            "{} files changed, recompiling {} active pages",
            changed_count,
            active_paths.len()
        );

        let mut outcomes = Vec::new();
        for path in active_paths {
            let (outcome, _) = self.compile_one_outcome(&path).await;
            outcomes.push(outcome);
        }
        self.last_active_recompile = Some(Instant::now());
        outcomes
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

                let (_, output_update) = self.refresh_atomic_css_output(Arc::clone(&config)).await;

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
                self.send_output_update(output_update).await;

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
    use crate::actor::messages::{CompilerMsg, VdomMsg, WsMsg};
    use crate::actor::vdom::VdomActor;
    use crate::address::SiteIndex;
    use crate::compiler::page::BUILD_CACHE;
    use crate::config::section::build::{HookConfig, WatchMode};
    use crate::config::{SiteConfig, config_handle, init_config};
    use crate::core::UrlPath;
    use crate::freshness::{build_hash_marker, compute_file_hash};
    use crate::page::PageRoute;
    use crate::reload::active::ACTIVE_PAGE;
    use crate::reload::queue::Priority;

    static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct ResetGlobals;

    impl Drop for ResetGlobals {
        fn drop(&mut self) {
            ACTIVE_PAGE.clear();
            crate::asset::version::clear();
            crate::freshness::clear_cache();
            init_config(SiteConfig::default());
        }
    }

    #[tokio::test]
    async fn direct_compile_invalidates_source_hash_before_noop_check() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::freshness::clear_cache();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let output = root.join("public");
        std::fs::create_dir_all(&content).unwrap();

        let source = content.join("index.md");
        let output_file = output.join("index.html");
        std::fs::write(&source, "# Old\n").unwrap();
        let old_hash = compute_file_hash(&source);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(&output_file, build_hash_marker(&old_hash, None)).unwrap();
        std::fs::write(&source, "# New\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output;
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );
        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);

        let background = actor.on_compile(queue, vec![source.clone()]).await;
        assert!(background.is_none());

        let mut saw_process = false;
        while let Ok(message) = vdom_rx.try_recv() {
            if let VdomMsg::Process { path, .. } = message
                && path == source
            {
                saw_process = true;
            }
        }
        assert!(
            saw_process,
            "a changed source must compile even if the old hash is still cached"
        );
    }

    #[tokio::test]
    async fn watched_post_hook_output_is_attached_to_page_process() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let output = root.join("public");
        std::fs::create_dir_all(&content).unwrap();

        let source = content.join("index.md");
        std::fs::write(&source, "# Home\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output;
        config.build.hooks.post.push(HookConfig {
            enable: true,
            name: Some("css".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "mkdir -p public/styles && printf updated > public/styles/site.css".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );
        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);

        let background = actor.on_compile(queue, vec![source.clone()]).await;
        assert!(background.is_none());

        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Process { path, assets, .. } => {
                assert_eq!(path, source);
                assert_eq!(assets.len(), 1);
                assert!(assets[0].starts_with("/styles/site.css?v="));
            }
            other => panic!("expected page process with post hook asset, got {other:?}"),
        }
        while let Ok(message) = vdom_rx.try_recv() {
            assert!(
                !matches!(message, VdomMsg::Asset { .. }),
                "post hook output must not be duplicated outside the page process"
            );
        }
    }

    #[tokio::test]
    async fn watched_pre_hook_source_asset_is_processed_before_pages() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let styles = root.join("assets/styles");
        let output = root.join("public");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::create_dir_all(output.join("styles")).unwrap();

        let source = content.join("index.md");
        std::fs::write(&source, "# Home\n").unwrap();
        std::fs::write(styles.join("tailwind.css"), "old").unwrap();
        std::fs::write(output.join("styles/tailwind.css"), "old").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output.clone();
        config.build.assets.nested = vec![crate::config::section::build::assets::NestedEntry::new(
            styles, "/styles",
        )];
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("css".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "printf compiled > assets/styles/tailwind.css".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, _vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );

        let background = actor
            .on_compile(CompileQueue::new(), vec![source.clone()])
            .await;

        assert!(background.is_none());
        assert_eq!(
            std::fs::read_to_string(output.join("styles/tailwind.css")).unwrap(),
            "compiled"
        );
    }

    #[tokio::test]
    async fn watched_pre_hook_public_asset_is_attached_to_page_process() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let styles = root.join("assets/styles");
        let output = root.join("public");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&styles).unwrap();

        let source = content.join("index.md");
        std::fs::write(&source, "# Home\n").unwrap();
        std::fs::write(styles.join("tailwind.css"), "@import \"tailwindcss\";").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output.clone();
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("css".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "mkdir -p public/styles && printf compiled > public/styles/tailwind.css".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );
        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);

        let background = actor.on_compile(queue, vec![source.clone()]).await;
        assert!(background.is_none());

        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Process { path, assets, .. } => {
                assert_eq!(path, source);
                assert_eq!(assets.len(), 1);
                assert!(assets[0].starts_with("/styles/tailwind.css?v="));
            }
            other => panic!("expected page process with pre hook asset, got {other:?}"),
        }
        while let Ok(message) = vdom_rx.try_recv() {
            assert!(
                !matches!(message, VdomMsg::Asset { .. }),
                "pre hook output must not be duplicated outside the page process"
            );
        }
    }

    #[tokio::test]
    async fn watched_pre_hook_stylesheet_output_and_page_change_is_one_transaction() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();
        BUILD_CACHE.clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let output = root.join("public");
        let output_css = output.join("styles/tailwind.css");
        let output_js = output.join("scripts/site.js");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(output_css.parent().unwrap()).unwrap();
        std::fs::create_dir_all(output_js.parent().unwrap()).unwrap();

        let source = content.join("index.md");
        std::fs::write(
            &source,
            "+++\ntitle = \"Home\"\n+++\n\n<p class=\"text-purple-300\">Home</p>\n",
        )
        .unwrap();
        std::fs::write(&output_css, ".text-purple-300{}").unwrap();
        std::fs::write(&output_js, "console.log('stable');").unwrap();

        let mut config = crate::config::test_parse_config(
            r#"[site.header]
no_fouc = false
styles = ["/styles/tailwind.css"]
scripts = ["/scripts/site.js"]
"#,
        );
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output;
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("css".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "cp content/index.md public/styles/tailwind.css".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let state = Arc::new(SiteIndex::new());
        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, vdom_rx) = mpsc::channel::<VdomMsg>(32);
        let (ws_tx, mut ws_rx) = mpsc::channel::<WsMsg>(32);
        let (vdom_actor, _, _, _) =
            VdomActor::new(vdom_rx, ws_tx, root.to_path_buf(), Arc::clone(&state));
        let vdom_handle = tokio::spawn(vdom_actor.run());
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx.clone(),
            config_handle(),
            Arc::clone(&state),
        );

        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);
        let background = actor.on_compile(queue, vec![source.clone()]).await;
        assert!(background.is_none());

        for _ in 0..4 {
            let message = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(
                message,
                WsMsg::Reload {
                    reason,
                    ..
                } if reason == "initial compile"
            ) {
                break;
            }
        }
        while ws_rx.try_recv().is_ok() {}

        std::fs::write(
            &source,
            "+++\ntitle = \"Home\"\n+++\n\n<p class=\"text-purple-700\">Home</p>\n",
        )
        .unwrap();

        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);
        let background = actor.on_compile(queue, vec![source]).await;
        assert!(background.is_none());

        let mut saw_patch = false;
        let mut saw_linked_asset = false;
        for _ in 0..4 {
            let Ok(Some(message)) =
                tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv()).await
            else {
                break;
            };

            match message {
                WsMsg::Asset { href } => {
                    panic!("stylesheet output duplicated outside the page patch: {href}");
                }
                WsMsg::Patch { assets, .. } => {
                    saw_patch = true;
                    saw_linked_asset = assets
                        .iter()
                        .any(|href| href.starts_with("/styles/tailwind.css?v="));
                }
                WsMsg::Reload { reason, .. } => panic!("class change must not reload: {reason}"),
                WsMsg::Error { path, error } => panic!("{path}: {error}"),
                WsMsg::ClearError { .. } | WsMsg::AddClient(_) | WsMsg::ClientConnected => {}
                WsMsg::Shutdown => {}
            }
        }

        assert!(saw_patch, "page class change should patch without reload");
        assert!(
            saw_linked_asset,
            "pre hook stylesheet output must travel with the page patch when the DOM patch does not already update it"
        );

        vdom_tx.send(VdomMsg::Shutdown).await.unwrap();
        vdom_handle.await.unwrap();
        BUILD_CACHE.clear();
    }

    #[tokio::test]
    async fn watched_pre_hook_output_event_does_not_send_duplicate_asset() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let styles = root.join("assets/styles");
        let output = root.join("public");
        let output_css = output.join("styles/tailwind.css");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::create_dir_all(output_css.parent().unwrap()).unwrap();

        let source = content.join("index.md");
        std::fs::write(&source, "# Home\n").unwrap();
        std::fs::write(styles.join("tailwind.css"), "@import \"tailwindcss\";").unwrap();
        std::fs::write(&output_css, "old").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output;
        config.build.assets.nested = vec![crate::config::section::build::assets::NestedEntry::new(
            styles, "/styles",
        )];
        config.site.header.styles = vec!["/styles/tailwind.css".into()];
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("css".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "printf compiled > public/styles/tailwind.css".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );
        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);

        let background = actor.on_compile(queue, vec![source.clone()]).await;
        assert!(background.is_none());

        let mut saw_asset = false;
        let mut saw_process = false;
        while let Ok(message) = vdom_rx.try_recv() {
            match message {
                VdomMsg::Process { path, assets, .. } if path == source => {
                    saw_process = true;
                    saw_asset = assets
                        .iter()
                        .any(|href| href.starts_with("/styles/tailwind.css?v="));
                }
                VdomMsg::Asset { href } if href.starts_with("/styles/tailwind.css?v=") => {
                    saw_asset = true;
                }
                _ => {}
            }
        }
        assert!(saw_asset);
        assert!(saw_process);

        actor.on_output_change(vec![output_css]).await;
        while let Ok(message) = vdom_rx.try_recv() {
            if let VdomMsg::Asset { href } = message {
                panic!("stale pre hook output event sent duplicate asset: {href}");
            }
        }
    }

    #[tokio::test]
    async fn watched_pre_hook_unlinked_output_is_sent_once_and_later_external_change_is_not_echo() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;
        crate::asset::version::clear();

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let content = root.join("content");
        let output = root.join("public");
        let output_json = output.join("data/site.json");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(output_json.parent().unwrap()).unwrap();

        let source = content.join("index.md");
        std::fs::write(&source, "# Home\n").unwrap();
        std::fs::write(&output_json, "{}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output;
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("data".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "printf '{\"updated\":true}' > public/data/site.json".into(),
            ],
            watch: WatchMode::Bool(true),
            quiet: true,
        });
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );
        let mut queue = CompileQueue::new();
        queue.add([source.clone()], Priority::Direct);

        let background = actor.on_compile(queue, vec![source]).await;
        assert!(background.is_none());
        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Process { assets, .. } => {
                assert_eq!(assets.len(), 1);
                assert!(assets[0].starts_with("/data/site.json?v="));
            }
            other => panic!("expected page process with internal pre hook output, got {other:?}"),
        }
        while vdom_rx.try_recv().is_ok() {}

        actor.on_output_change(vec![output_json.clone()]).await;
        while let Ok(message) = vdom_rx.try_recv() {
            if let VdomMsg::Asset { href } = message {
                panic!("stale pre hook output event sent duplicate asset: {href}");
            }
        }

        std::fs::write(&output_json, r#"{"updated":false}"#).unwrap();
        actor.on_output_change(vec![output_json]).await;
        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Asset { href } => assert!(href.starts_with("/data/site.json?v=")),
            other => panic!("expected later external output asset, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn atomic_css_only_change_recompiles_active_pages() {
        let _guard = TEST_LOCK.lock().await;
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

    #[tokio::test]
    async fn output_css_change_sends_asset_update() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;

        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let css = output.join("styles/tailwind.css");
        std::fs::create_dir_all(css.parent().unwrap()).unwrap();
        std::fs::write(&css, "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(temp.path());
        config.build.output = output;
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );

        actor.on_output_change(vec![css]).await;

        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Asset { href } => {
                assert!(href.starts_with("/styles/tailwind.css?v="));
            }
            other => panic!("expected asset update, got {other:?}"),
        }
        assert!(vdom_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn output_non_css_change_sends_asset_update() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;

        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let js = output.join("scripts/app.js");
        std::fs::create_dir_all(js.parent().unwrap()).unwrap();
        std::fs::write(&js, "console.log(1)").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(temp.path());
        config.build.output = output;
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );

        actor.on_output_change(vec![js]).await;

        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Asset { href } => {
                assert!(href.starts_with("/scripts/app.js?v="));
            }
            other => panic!("expected asset update, got {other:?}"),
        }
        assert!(vdom_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn removed_output_asset_sends_reload() {
        let _guard = TEST_LOCK.lock().await;
        let _reset = ResetGlobals;

        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let css = output.join("styles/tailwind.css");

        let mut config = SiteConfig::default();
        config.set_root(temp.path());
        config.build.output = output;
        init_config(config);

        let (_compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(1);
        let (vdom_tx, mut vdom_rx) = mpsc::channel::<VdomMsg>(8);
        let mut actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );

        actor.on_output_change(vec![css]).await;

        match vdom_rx.recv().await.unwrap() {
            VdomMsg::Reload { reason } => {
                assert!(reason.contains("output assets updated"));
            }
            other => panic!("expected reload, got {other:?}"),
        }
        assert!(vdom_rx.try_recv().is_err());
    }
}
