//! Compiler Actor - Typst Compilation Wrapper
//!
//! Handles file compilation with priority-based scheduling:
//! - Direct/Active files: compiled immediately for instant feedback
//! - Affected files: compiled in background, interruptible by new requests

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::messages::{CompilerMsg, VdomMsg};
use crate::address::{GLOBAL_ADDRESS_SPACE, PermalinkUpdate};
use crate::config::SiteConfig;
use crate::page::STORED_PAGES;
use crate::reload::classify::{collect_dependents, url_to_content_path};
use crate::reload::compile::{CompileOutcome, compile_page};

/// Result of background compilation
struct BatchResult {
    outcomes: Vec<CompileOutcome>,
    pages_hash: u64,
}

type BackgroundTask = JoinHandle<BatchResult>;
const ACTIVE_RECOMPILE_COOLDOWN: Duration = Duration::from_millis(250);

pub struct CompilerActor {
    rx: mpsc::Receiver<CompilerMsg>,
    vdom_tx: mpsc::Sender<VdomMsg>,
    config: Arc<SiteConfig>,
    last_active_recompile: Option<Instant>,
}

impl CompilerActor {
    pub fn new(
        rx: mpsc::Receiver<CompilerMsg>,
        vdom_tx: mpsc::Sender<VdomMsg>,
        config: Arc<SiteConfig>,
    ) -> Self {
        Self {
            rx,
            vdom_tx,
            config,
            last_active_recompile: None,
        }
    }

    /// Main event loop with interruptible background compilation
    pub async fn run(mut self) {
        let mut background: Option<BackgroundTask> = None;

        loop {
            tokio::select! {
                biased; // New messages take priority

                Some(msg) = self.rx.recv() => {
                    // Interrupt background on new compile request
                    if matches!(msg, CompilerMsg::Compile { .. }) {
                        abort_task(&mut background);
                    }
                    background = self.dispatch(msg, background).await;
                }

                result = wait_task(&mut background) => {
                    background = None;
                    self.on_background_done(result).await;
                }
            }
        }
    }

    /// Dispatch message to handler
    async fn dispatch(
        &mut self,
        msg: CompilerMsg,
        bg: Option<BackgroundTask>,
    ) -> Option<BackgroundTask> {
        match msg {
            CompilerMsg::Compile {
                queue,
                changed_paths,
            } => self.on_compile(queue, changed_paths).await,
            CompilerMsg::CompileDependents(deps) => {
                self.on_compile_dependents(deps).await;
                bg
            }
            CompilerMsg::ContentCreated(paths) => {
                self.on_content_created(paths).await;
                bg
            }
            CompilerMsg::ContentRemoved(paths) => {
                self.on_content_removed(paths).await;
                bg
            }
            CompilerMsg::AssetChange(paths) => {
                self.on_asset_change(paths).await;
                bg
            }
            CompilerMsg::OutputChange(paths) => {
                self.on_output_change(paths).await;
                bg
            }
            CompilerMsg::RetryScan { changed_paths } => {
                abort_task(&mut { bg });
                self.on_retry_scan(changed_paths).await;
                None
            }
            CompilerMsg::FullRebuild => {
                abort_task(&mut { bg });
                self.on_full_rebuild().await;
                None
            }
            CompilerMsg::Shutdown => {
                crate::log!("compile"; "shutting down");
                bg
            }
        }
    }
}

impl CompilerActor {
    /// Handle compile request: run hooks first, then compile
    async fn on_compile(
        &mut self,
        queue: crate::reload::queue::CompileQueue,
        changed_paths: Vec<PathBuf>,
    ) -> Option<BackgroundTask> {
        let start = Instant::now();
        let pages_hash = STORED_PAGES.pages_hash();

        // Run pre hooks BEFORE compilation (blocking)
        // This ensures CSS/JS assets are ready before pages are compiled
        let hook_outputs_changed = !changed_paths.is_empty() && self.run_pre_hooks(&changed_paths);
        let changed_set: FxHashSet<PathBuf> = changed_paths.iter().cloned().collect();

        // Direct/Active - compile immediately
        let direct: Vec<_> = queue.direct_files().cloned().collect();
        for path in &direct {
            if self.should_skip_noop_change(path, &changed_set, hook_outputs_changed) {
                crate::debug!("compile"; "skip no-op save: {}", path.display());
                continue;
            }
            self.compile_one(path).await;
        }

        // Affected - compile in background
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
    /// Any executed hook is treated as potentially side-effecting.
    fn run_pre_hooks(&self, changed_paths: &[PathBuf]) -> bool {
        use crate::hooks;

        let refs: Vec<&Path> = changed_paths.iter().map(|p| p.as_path()).collect();
        hooks::run_watched_hooks(&self.config, &refs) > 0
    }

    /// Skip recompilation for no-op saves:
    /// - file was directly reported as changed
    /// - no watched hook ran for this change
    /// - output still contains matching source hash marker
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

    /// Handle background task completion
    async fn on_background_done(&mut self, result: BatchResult) {
        let start = Instant::now();

        for outcome in result.outcomes {
            self.route(outcome).await;
        }

        self.finish_batch(result.pages_hash).await;
        crate::debug!("compile"; "background done in {:?}", start.elapsed());
    }

    /// Finalize a compilation batch
    async fn finish_batch(&mut self, hash_before: u64) {
        // Recompile virtual package users if metadata changed
        if STORED_PAGES.pages_hash() != hash_before {
            self.recompile_virtual_users().await;
        }
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
    }

    async fn on_compile_dependents(&mut self, deps: Vec<PathBuf>) {
        let affected = collect_dependents(&deps);
        if affected.is_empty() {
            crate::log!("compile"; "no dependents for {} deps", deps.len());
        } else {
            self.compile_batch_blocking(affected).await;
        }
    }

    /// Handle new content files — compile and register
    async fn on_content_created(&mut self, paths: Vec<PathBuf>) {
        let count = paths.len();
        crate::debug!("watch"; "{} new content files", count);

        let pages_hash = STORED_PAGES.pages_hash();

        for path in &paths {
            self.compile_one(path).await;
        }

        self.finish_batch(pages_hash).await;
    }

    /// Handle deleted content files — cleanup all state
    async fn on_content_removed(&mut self, paths: Vec<PathBuf>) {
        use crate::compiler::dependency::remove_content;
        use crate::compiler::page::BUILD_CACHE;
        use tola_vdom::CacheKey;

        let count = paths.len();
        crate::debug!("watch"; "{} content files removed", count);

        for path in &paths {
            // Look up URL before removing mappings
            let url = GLOBAL_ADDRESS_SPACE.read().url_for_source(path).cloned();

            // Untrack from all stores
            GLOBAL_ADDRESS_SPACE.write().remove_by_source(path);
            STORED_PAGES.remove_by_source(path);
            remove_content(path);

            if let Some(url) = &url {
                BUILD_CACHE.remove(&CacheKey::new(url.as_str()));
                self.clean_output_file(url);
                crate::debug!("watch"; "cleaned up {} -> {}", path.display(), url);
            }
        }

        // Recompile pages that use @tola/pages (they need updated page list)
        self.recompile_virtual_users().await;

        // Signal batch end to flush any pending output
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
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

        // Remove empty parent directory
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

    async fn on_asset_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let config = Arc::clone(&self.config);
        let count = paths.len();

        // Process assets (copy to output)
        let errors = tokio::task::spawn_blocking({
            let paths = paths.clone();
            move || process_assets(&paths, &config)
        })
        .await
        .unwrap_or_default();

        log_asset_errors(&errors);

        // Update asset versions
        let mut any_changed = false;
        for path in &paths {
            if version::update_version(path) {
                any_changed = true;
            }
        }

        // If versions changed, recompile active pages (VDOM Patch, no reload)
        if any_changed {
            self.recompile_active_pages("asset", count).await;
        } else if !errors.is_empty() {
            // Only errors, no version changes - still notify
            let reason = format_asset_reason(count, errors.len());
            let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
        }
    }

    async fn on_output_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;

        let total = paths.len();
        // Only treat non-HTML output files as asset-like changes.
        // HTML outputs are generated by normal page compilation and should not
        // trigger recursive active-page recompilation.
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
                // Removed output file: treat as changed so active pages can refresh.
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

        // Flush batch log
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
    }

    fn should_throttle_active_recompile(&self) -> bool {
        self.last_active_recompile
            .is_some_and(|last| last.elapsed() < ACTIVE_RECOMPILE_COOLDOWN)
    }

    async fn on_full_rebuild(&mut self) {
        use crate::asset::version;
        use crate::compiler::dependency::clear_graph;
        use crate::config::{clear_clean_flag, reload_config};
        use crate::core::{BuildMode, set_healthy};
        use crate::reload::active::ACTIVE_PAGE;

        crate::debug!("compile"; "full rebuild triggered");

        if let Ok(true) = reload_config() {
            self.config = crate::config::cfg();
        }

        // Clear caches
        clear_graph();
        version::clear();
        let _ = self.vdom_tx.send(VdomMsg::Clear).await;

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

                // Recompile active pages for VDOM Patch (no reload!)
                let active_urls = ACTIVE_PAGE.get_all();
                if !active_urls.is_empty() {
                    crate::debug!("compile"; "recompiling {} active pages after rebuild", active_urls.len());
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
                let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
            }
            Err(e) => {
                crate::debug!("compile"; "spawn_blocking error: {}", e);
                let reason = format!("internal error: {}", e);
                let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
            }
        }
    }

    /// Retry scan after initial failure, then compile changed files
    async fn on_retry_scan(&mut self, changed_paths: Vec<PathBuf>) {
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

                // Compile changed content files
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

                // Also compile active pages if not in changed_paths
                let active_urls = ACTIVE_PAGE.get_all();
                for url in active_urls {
                    if let Some(path) = url_to_content_path(url.as_str(), &self.config)
                        && !content_files.contains(&path)
                    {
                        self.compile_one(&path).await;
                    }
                }

                // Mark healthy only after retry compilation finishes.
                set_healthy(true);
                let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
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

impl CompilerActor {
    /// Compile a single file (blocking)
    async fn compile_one(&mut self, path: &Path) {
        let config = Arc::clone(&self.config);
        let path = path.to_path_buf();

        let result = tokio::task::spawn_blocking(move || {
            let outcome = compile_page(&path, &config);
            // Use flush_current_thread_deps (not flush_thread_local_deps) because
            // spawn_blocking threads are not rayon workers, so rayon::broadcast
            // won't reach them. This is consistent with scheduler's do_compile.
            crate::compiler::dependency::flush_current_thread_deps();
            outcome
        })
        .await;

        match result {
            Ok(outcome) => self.route(outcome).await,
            Err(e) => crate::log!("compile"; "error: {}", e),
        }
    }

    /// Compile multiple files in parallel (blocking)
    async fn compile_batch_blocking(&mut self, paths: Vec<PathBuf>) {
        let outcomes = compile_batch(paths, Arc::clone(&self.config)).await;
        for outcome in outcomes {
            self.route(outcome).await;
        }
    }

    /// Recompile pages using @tola/* virtual packages
    async fn recompile_virtual_users(&mut self) {
        use crate::compiler::dependency::collect_virtual_dependents;

        let all_dependents = collect_virtual_dependents();

        if !all_dependents.is_empty() {
            crate::debug!("compile"; "recompiling {} virtual package users", all_dependents.len());
            self.compile_batch_blocking(all_dependents.into_iter().collect())
                .await;
        } else {
            crate::debug!("compile"; "no virtual package users to recompile");
        }
    }

    /// Route compilation outcome to VdomActor
    async fn route(&mut self, outcome: CompileOutcome) {
        let msg = match outcome {
            CompileOutcome::Vdom {
                path,
                url_path,
                vdom,
                warnings,
            } => {
                let permalink_change = update_address_space(&path, &url_path);
                VdomMsg::Process {
                    path,
                    url_path,
                    vdom,
                    permalink_change,
                    warnings,
                }
            }
            CompileOutcome::Reload { reason } => VdomMsg::Reload { reason },
            CompileOutcome::Skipped => VdomMsg::Skip,
            CompileOutcome::Error {
                path,
                url_path,
                error,
            } => VdomMsg::Error {
                path,
                url_path: url_path.unwrap_or_default(),
                error,
            },
        };
        let _ = self.vdom_tx.send(msg).await;
    }
}

/// Spawn background compilation task
fn spawn_batch(paths: Vec<PathBuf>, config: Arc<SiteConfig>, pages_hash: u64) -> BackgroundTask {
    tokio::spawn(async move {
        let outcomes = compile_batch(paths, config).await;
        BatchResult {
            outcomes,
            pages_hash,
        }
    })
}

/// Compile files in parallel using rayon
async fn compile_batch(paths: Vec<PathBuf>, config: Arc<SiteConfig>) -> Vec<CompileOutcome> {
    use rayon::prelude::*;

    tokio::task::spawn_blocking(move || {
        let results: Vec<_> = paths
            .par_iter()
            .map(|path| compile_page(path, &config))
            .collect();
        crate::compiler::dependency::flush_thread_local_deps();
        results
    })
    .await
    .unwrap_or_default()
}

/// Abort background task if running
fn abort_task(task: &mut Option<BackgroundTask>) {
    if let Some(t) = task.take() {
        t.abort();
        crate::debug!("compile"; "interrupted background task");
    }
}

/// Wait for background task (blocks forever if None)
async fn wait_task(task: &mut Option<BackgroundTask>) -> BatchResult {
    match task.take() {
        Some(handle) => handle.await.unwrap_or(BatchResult {
            outcomes: vec![],
            pages_hash: 0,
        }),
        None => std::future::pending().await,
    }
}

fn update_address_space(path: &Path, url_path: &crate::core::UrlPath) -> Option<PermalinkUpdate> {
    let mut space = GLOBAL_ADDRESS_SPACE.write();
    let update = space.update_source_url(path, url_path);
    crate::debug!("permalink"; "update({}, {}) = {:?}", path.display(), url_path, update);
    match update {
        PermalinkUpdate::Unchanged => None,
        _ => Some(update),
    }
}

fn process_assets(paths: &[PathBuf], config: &SiteConfig) -> Vec<(PathBuf, String)> {
    use crate::asset::{process_asset, process_rel_asset};

    paths
        .iter()
        .filter_map(|path| {
            let result = if config.build.assets.contains_source(path) {
                process_asset(path, config, false, true)
            } else if path.starts_with(&config.build.content) {
                process_rel_asset(path, config, false, true)
            } else {
                process_asset(path, config, false, true)
            };
            result.err().map(|e| (path.clone(), e.to_string()))
        })
        .collect()
}

fn log_asset_errors(errors: &[(PathBuf, String)]) {
    for (path, error) in errors {
        crate::log!("error"; "asset {}: {}", path.display(), error);
    }
}

fn is_reloadable_output_asset(path: &Path) -> bool {
    !matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    )
}

fn format_asset_reason(total: usize, error_count: usize) -> String {
    if error_count == 0 {
        format!("{} assets updated", total)
    } else {
        format!(
            "{} assets updated, {} errors",
            total - error_count,
            error_count
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_outcome_variants() {
        let _ = CompileOutcome::Reload {
            reason: "test".into(),
        };
        let _ = CompileOutcome::Skipped;
        let _ = CompileOutcome::Error {
            path: PathBuf::from("/test.typ"),
            url_path: None,
            error: "test".into(),
        };
    }

    #[test]
    fn test_is_reloadable_output_asset() {
        assert!(is_reloadable_output_asset(Path::new(
            "/public/assets/app.css"
        )));
        assert!(is_reloadable_output_asset(Path::new(
            "/public/assets/app.js"
        )));
        assert!(!is_reloadable_output_asset(Path::new(
            "/public/page/index.html"
        )));
        assert!(!is_reloadable_output_asset(Path::new(
            "/public/page/index.htm"
        )));
    }
}
