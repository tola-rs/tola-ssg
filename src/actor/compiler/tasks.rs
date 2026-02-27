use std::path::PathBuf;
use std::sync::Arc;

use crate::config::SiteConfig;
use crate::reload::compile::{CompileOutcome, compile_page};

use super::{BackgroundTask, BatchResult};

/// Spawn background compilation task.
pub(super) fn spawn_batch(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    pages_hash: u64,
    watched_post_paths: Option<Vec<PathBuf>>,
) -> BackgroundTask {
    tokio::spawn(async move {
        let outcomes = compile_batch(paths, config).await;
        BatchResult {
            outcomes,
            pages_hash,
            watched_post_paths,
        }
    })
}

/// Compile files in parallel using rayon.
pub(super) async fn compile_batch(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
) -> Vec<CompileOutcome> {
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

/// Abort background task if running.
pub(super) fn abort_task(task: &mut Option<BackgroundTask>) {
    if let Some(t) = task.take() {
        t.abort();
        crate::debug!("compile"; "interrupted background task");
    }
}

/// Wait for background task (blocks forever if None).
pub(super) async fn wait_task(task: &mut Option<BackgroundTask>) -> BatchResult {
    match task.take() {
        Some(handle) => handle.await.unwrap_or(BatchResult {
            outcomes: vec![],
            pages_hash: 0,
            watched_post_paths: None,
        }),
        None => std::future::pending().await,
    }
}
