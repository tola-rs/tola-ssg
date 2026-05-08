use std::path::PathBuf;
use std::time::Instant;

use super::tasks::{abort_task, wait_task};
use super::{BackgroundTask, BatchResult, CompilerActor};
use crate::actor::messages::{CompilerMsg, VdomMsg};

impl CompilerActor {
    /// Main event loop with interruptible background compilation
    pub async fn run(mut self) {
        let mut background: Option<BackgroundTask> = None;

        loop {
            tokio::select! {
                biased;

                msg = self.rx.recv() => {
                    let Some(msg) = msg else {
                        abort_task(&mut background);
                        break;
                    };

                    let is_shutdown = matches!(msg, CompilerMsg::Shutdown);
                    if background.is_some() && interrupts_background(&msg) {
                        self.page_epoch.advance();
                        abort_task(&mut background);
                    }
                    background = self.dispatch(msg, background).await;
                    if is_shutdown {
                        abort_task(&mut background);
                        break;
                    }
                }

                result = wait_task(&mut background), if background.is_some() => {
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
                self.on_retry_scan(changed_paths).await;
                None
            }
            CompilerMsg::FullRebuild => {
                self.on_full_rebuild().await;
                None
            }
            CompilerMsg::Shutdown => bg,
        }
    }

    /// Handle background task completion
    async fn on_background_done(&mut self, result: BatchResult) {
        let start = Instant::now();

        for outcome in result.outcomes {
            self.route(outcome).await;
        }

        self.finish_batch(result.pages_hash, result.watched_post_paths)
            .await;
        crate::debug!("compile"; "background done in {:?}", start.elapsed());
    }

    /// Finalize a compilation batch
    pub(super) async fn finish_batch(
        &mut self,
        hash_before: u64,
        watched_post_paths: Option<Vec<PathBuf>>,
    ) {
        if self.store.pages_hash() != hash_before {
            self.recompile_virtual_users().await;
        }
        if let Some(paths) = watched_post_paths {
            self.run_watched_post_hooks(&paths);
        }
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
    }
}

fn interrupts_background(msg: &CompilerMsg) -> bool {
    matches!(
        msg,
        CompilerMsg::Compile { .. }
            | CompilerMsg::CompileDependents(_)
            | CompilerMsg::ContentCreated(_)
            | CompilerMsg::ContentRemoved(_)
            | CompilerMsg::RetryScan { .. }
            | CompilerMsg::FullRebuild
            | CompilerMsg::Shutdown
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::mpsc;

    use super::*;
    use crate::config::SiteConfig;
    use crate::page::StoredPageMap;

    #[tokio::test]
    async fn exits_on_shutdown_message() {
        let (compiler_tx, compiler_rx) = mpsc::channel(1);
        let (vdom_tx, _vdom_rx) = mpsc::channel::<VdomMsg>(1);
        let actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            Arc::new(SiteConfig::default()),
            Arc::new(StoredPageMap::new()),
        );

        let handle = tokio::spawn(actor.run());
        compiler_tx.send(CompilerMsg::Shutdown).await.unwrap();

        let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(result.is_ok(), "compiler actor should exit after shutdown");
    }
}
