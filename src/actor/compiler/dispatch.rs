use std::time::Instant;

use super::tasks::{abort_task, wait_task};
use super::{BackgroundTask, BatchResult, CompilerActor};
use crate::actor::messages::{CompilerMsg, VdomMsg};
use crate::page::STORED_PAGES;

impl CompilerActor {
    /// Main event loop with interruptible background compilation
    pub async fn run(mut self) {
        let mut background: Option<BackgroundTask> = None;

        loop {
            tokio::select! {
                biased;

                Some(msg) = self.rx.recv() => {
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
        mut bg: Option<BackgroundTask>,
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
                abort_task(&mut bg);
                self.on_retry_scan(changed_paths).await;
                None
            }
            CompilerMsg::FullRebuild => {
                abort_task(&mut bg);
                self.on_full_rebuild().await;
                None
            }
            CompilerMsg::Shutdown => {
                crate::log!("compile"; "shutting down");
                bg
            }
        }
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
    pub(super) async fn finish_batch(&mut self, hash_before: u64) {
        if STORED_PAGES.pages_hash() != hash_before {
            self.recompile_virtual_users().await;
        }
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd).await;
    }
}
