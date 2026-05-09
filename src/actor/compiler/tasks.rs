use std::path::PathBuf;
use std::sync::Arc;

use crate::address::SiteIndex;
use crate::compiler::dependency::flush_thread_local_deps;
use crate::compiler::page::{PageStateTicket, TypstHost};
use crate::compiler::scheduler::SCHEDULER;
use crate::config::SiteConfig;
use crate::reload::compile::{CompileOutcome, compile_page, compile_page_with_ticket};

use super::{BackgroundTask, BatchResult};

/// Spawn background compilation task.
pub(super) fn spawn_batch(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    typst_host: Arc<TypstHost>,
    state: Arc<SiteIndex>,
    pages_hash: u64,
    watched_post_paths: Option<Vec<PathBuf>>,
    ticket: PageStateTicket,
) -> BackgroundTask {
    tokio::spawn(async move {
        let outcomes =
            compile_batch_with_ticket(paths, Arc::clone(&config), typst_host, state, ticket).await;
        BatchResult {
            config,
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
    typst_host: Arc<TypstHost>,
    state: Arc<SiteIndex>,
) -> Vec<CompileOutcome> {
    compile_batch_inner(paths, config, typst_host, state, None).await
}

pub(super) async fn compile_batch_with_ticket(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    typst_host: Arc<TypstHost>,
    state: Arc<SiteIndex>,
    ticket: PageStateTicket,
) -> Vec<CompileOutcome> {
    compile_batch_inner(paths, config, typst_host, state, Some(ticket)).await
}

async fn compile_batch_inner(
    paths: Vec<PathBuf>,
    config: Arc<SiteConfig>,
    typst_host: Arc<TypstHost>,
    state: Arc<SiteIndex>,
    ticket: Option<PageStateTicket>,
) -> Vec<CompileOutcome> {
    use rayon::prelude::*;
    for path in &paths {
        SCHEDULER.invalidate(path);
    }

    tokio::task::spawn_blocking(move || {
        let results: Vec<_> = paths
            .par_iter()
            .map(|path| match &ticket {
                Some(ticket) => {
                    compile_page_with_ticket(path, &config, &typst_host, &state, ticket)
                }
                None => compile_page(path, &config, &typst_host, &state),
            })
            .collect();
        flush_thread_local_deps();
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
            config: Arc::new(SiteConfig::default()),
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

    fn reset_state(state: &SiteIndex) {
        state.clear();
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

        let state = Arc::new(SiteIndex::new());
        reset_state(&state);

        let epoch = PageStateEpoch::new();
        let ticket = epoch.ticket();
        epoch.advance();

        let config = Arc::new(config);
        let typst_host = Arc::new(TypstHost::for_config(&config));
        let outcomes = compile_batch_with_ticket(
            vec![page.clone()],
            config,
            typst_host,
            Arc::clone(&state),
            ticket,
        )
        .await;

        assert!(matches!(outcomes.as_slice(), [CompileOutcome::Skipped]));
        assert!(state.with_pages(|pages| pages.get_permalink_by_source(&page).is_none()));
        assert!(state.with_pages(|pages| {
            pages
                .get_pages_with_drafts()
                .iter()
                .all(|page| page.permalink != UrlPath::from_page("/post/"))
        }));

        reset_state(&state);
    }
}
