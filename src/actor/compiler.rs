//! Compiler Actor - Typst Compilation Wrapper
//!
//! Handles file compilation with priority-based scheduling:
//! - Direct/Active files: compiled immediately for instant feedback
//! - Affected files: compiled in background, interruptible by new requests

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

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

pub struct CompilerActor {
    rx: mpsc::Receiver<CompilerMsg>,
    vdom_tx: mpsc::Sender<VdomMsg>,
    config: Arc<SiteConfig>,
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
            CompilerMsg::Compile { queue, changed_paths } => {
                self.on_compile(queue, changed_paths).await
            }
            CompilerMsg::CompileDependents(deps) => {
                self.on_compile_dependents(deps).await;
                bg
            }
            CompilerMsg::AssetChange(paths) => {
                self.on_asset_change(paths).await;
                bg
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
        if !changed_paths.is_empty() {
            self.run_pre_hooks(&changed_paths);
        }

        // Direct/Active - compile immediately
        let direct: Vec<_> = queue.direct_files().cloned().collect();
        for path in &direct {
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

    /// Run pre hooks that match changed files and update asset versions
    fn run_pre_hooks(&self, changed_paths: &[PathBuf]) {
        use crate::asset::version;
        use crate::hooks;

        let refs: Vec<&Path> = changed_paths.iter().map(|p| p.as_path()).collect();
        let executed = hooks::run_watched_hooks(&self.config, &refs);

        // If hooks ran, update CSS output version immediately
        // so subsequent compiles use the new asset version
        if executed > 0 {
            if let Some(css_output) = self.get_css_output_path() {
                version::update_version(&css_output);
            }
        }
    }

    /// Get CSS processor output path if enabled
    fn get_css_output_path(&self) -> Option<PathBuf> {
        if !self.config.build.hooks.css.enable {
            return None;
        }
        self.config
            .build
            .hooks
            .css
            .path
            .as_ref()
            .and_then(|p| crate::asset::route_from_source(p.clone(), &self.config).ok())
            .map(|r| r.output)
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

    async fn on_asset_change(&mut self, paths: Vec<PathBuf>) {
        use crate::asset::version;
        use crate::reload::active::ACTIVE_PAGE;

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
            let active_urls = ACTIVE_PAGE.get_all();
            if !active_urls.is_empty() {
                crate::log!("asset"; "{} assets changed, recompiling {} active pages", count, active_urls.len());
                for url in active_urls {
                    if let Some(path) = url_to_content_path(url.as_str(), &self.config) {
                        self.compile_one(&path).await;
                    }
                }
                // Flush batch log
                let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
            }
        } else if !errors.is_empty() {
            // Only errors, no version changes - still notify
            let reason = format_asset_reason(count, errors.len());
            let _ = self.vdom_tx.send(VdomMsg::Reload { reason }).await;
        }
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
                    crate::log!("compile"; "recompiling {} active pages after rebuild", active_urls.len());
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
}

impl CompilerActor {
    /// Compile a single file (blocking)
    async fn compile_one(&mut self, path: &Path) {
        let config = Arc::clone(&self.config);
        let path = path.to_path_buf();

        let result = tokio::task::spawn_blocking(move || {
            let outcome = compile_page(&path, &config);
            crate::compiler::dependency::flush_thread_local_deps();
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
        use crate::compiler::dependency::get_dependents;
        use crate::package::TolaPackage;
        use rustc_hash::FxHashSet;

        // Collect dependents from all virtual packages
        let all_dependents: FxHashSet<_> = TolaPackage::all()
            .iter()
            .flat_map(|pkg| get_dependents(&pkg.sentinel()))
            .collect();

        if !all_dependents.is_empty() {
            crate::log!("compile"; "recompiling {} virtual package users", all_dependents.len());
            self.compile_batch_blocking(all_dependents.into_iter().collect())
                .await;
        }
    }

    /// Route compilation outcome to VdomActor
    async fn route(&mut self, outcome: CompileOutcome) {
        let msg = match outcome {
            CompileOutcome::Vdom {
                path,
                url_path,
                vdom,
            } => {
                let permalink_change = update_address_space(&path, &url_path);
                VdomMsg::Process {
                    path,
                    url_path,
                    vdom,
                    permalink_change,
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
}
