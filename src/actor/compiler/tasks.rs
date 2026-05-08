use std::path::PathBuf;
use std::sync::Arc;

use crate::compiler::page::PageStateTicket;
use crate::config::SiteConfig;
use crate::reload::compile::{CompileOutcome, compile_page, compile_page_with_ticket};

use super::{BackgroundTask, BatchResult};

/// Spawn background compilation task.
pub(super) fn spawn_batch(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    pages_hash: u64,
    watched_post_paths: Option<Vec<PathBuf>>,
    ticket: PageStateTicket,
) -> BackgroundTask {
    tokio::spawn(async move {
        let outcomes = compile_batch_with_ticket(paths, config, ticket).await;
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
    compile_batch_inner(paths, config, None).await
}

pub(super) async fn compile_batch_with_ticket(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    ticket: PageStateTicket,
) -> Vec<CompileOutcome> {
    compile_batch_inner(paths, config, Some(ticket)).await
}

async fn compile_batch_inner(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    ticket: Option<PageStateTicket>,
) -> Vec<CompileOutcome> {
    use rayon::prelude::*;
    for path in &paths {
        crate::compiler::scheduler::SCHEDULER.invalidate(path);
    }

    tokio::task::spawn_blocking(move || {
        let results: Vec<_> = paths
            .par_iter()
            .map(|path| match &ticket {
                Some(ticket) => compile_page_with_ticket(path, &config, ticket),
                None => compile_page(path, &config),
            })
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::*;
    use crate::compiler::page::PageStateEpoch;
    use crate::core::UrlPath;

    fn reset_global_state() {
        crate::page::STORED_PAGES.clear();
        crate::address::GLOBAL_ADDRESS_SPACE.write().clear();
    }

    #[tokio::test]
    async fn compile_batch_with_stale_ticket_does_not_commit_page_state() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&content_dir).unwrap();

        let page = content_dir.join("post.md");
        fs::write(&page, "+++\ntitle = \"Post\"\n+++\n\n# Post\n").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;
        config.build.output = output_dir;

        reset_global_state();

        let epoch = PageStateEpoch::new();
        let ticket = epoch.ticket();
        epoch.advance();

        let outcomes =
            compile_batch_with_ticket(vec![page.clone()], Arc::new(config), ticket).await;

        assert!(matches!(outcomes.as_slice(), [CompileOutcome::Skipped]));
        assert!(
            crate::page::STORED_PAGES
                .get_permalink_by_source(&page)
                .is_none()
        );
        assert!(
            crate::page::STORED_PAGES
                .get_pages_with_drafts()
                .iter()
                .all(|page| page.permalink != UrlPath::from_page("/post/"))
        );

        reset_global_state();
    }
}
