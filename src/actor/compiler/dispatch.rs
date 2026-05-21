use std::path::PathBuf;
use std::time::Instant;

use super::tasks::{abort_task, wait_task};
use super::{BackgroundTask, BatchResult, CompilerActor};
use crate::actor::messages::{CompilerMsg, VdomMsg};
use crate::reload::output;

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

        self.finish_batch(
            result.config,
            result.pages_hash,
            result.watched_post_paths,
            result.output_update,
            result.outcomes,
        )
        .await;
        crate::debug!("compile"; "background done in {:?}", start.elapsed());
    }

    /// Finalize a compilation batch
    pub(super) async fn finish_batch(
        &mut self,
        config: std::sync::Arc<crate::config::SiteConfig>,
        hash_before: u64,
        watched_post_paths: Option<Vec<PathBuf>>,
        mut output_update: output::Update,
        outcomes: Vec<crate::reload::compile::CompileOutcome>,
    ) {
        if self.state.with_pages(|pages| pages.pages_hash()) != hash_before {
            self.recompile_virtual_users().await;
        }
        if let Some(paths) = watched_post_paths {
            let before = output::snapshot(&config);
            match self.run_watched_post_hooks(&paths) {
                Ok(executed) => {
                    if executed > 0 {
                        let changed = output::changed(&before, &config);
                        if !changed.is_empty() {
                            output_update.extend(self.stage_output_change(changed));
                        }
                    }
                }
                Err(e) => {
                    self.report_hook_error(
                        std::sync::Arc::clone(&config),
                        crate::hooks::HookPhase::Post,
                        e,
                    )
                    .await;
                    let _ = self.vdom_tx.send(VdomMsg::BatchEnd { config }).await;
                    return;
                }
            }
        }
        let has_page_outcome = outcomes
            .iter()
            .any(|outcome| matches!(outcome, crate::reload::compile::CompileOutcome::Vdom { .. }));
        let assets = if output_update.reload_count() == 0 && has_page_outcome {
            output_update.hrefs().to_vec()
        } else {
            Vec::new()
        };
        let send_standalone_output = output_update.reload_count() > 0 || !has_page_outcome;

        self.route_all(outcomes, std::sync::Arc::clone(&config), &assets)
            .await;
        if send_standalone_output {
            self.send_output_update(output_update).await;
        }
        self.write_seo_outputs(std::sync::Arc::clone(&config)).await;
        let _ = self.vdom_tx.send(VdomMsg::BatchEnd { config }).await;
    }

    pub(super) async fn write_seo_outputs(
        &self,
        config: std::sync::Arc<crate::config::SiteConfig>,
    ) {
        if !config.site.seo.has_feed_outputs() && !config.site.seo.sitemap.enable {
            return;
        }

        let state = std::sync::Arc::clone(&self.state);
        match tokio::task::spawn_blocking(move || crate::seo::build_outputs(&config, &state)).await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => crate::log!("warning"; "failed to write SEO outputs: {}", e),
            Err(e) => crate::debug!("compile"; "SEO output task failed: {}", e),
        }
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
    use crate::address::SiteIndex;
    use crate::config::config_handle;

    #[tokio::test]
    async fn exits_on_shutdown_message() {
        let (compiler_tx, compiler_rx) = mpsc::channel(1);
        let (vdom_tx, _vdom_rx) = mpsc::channel::<VdomMsg>(1);
        let actor = CompilerActor::new(
            compiler_rx,
            vdom_tx,
            config_handle(),
            Arc::new(SiteIndex::new()),
        );

        let handle = tokio::spawn(actor.run());
        compiler_tx.send(CompilerMsg::Shutdown).await.unwrap();

        let result = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(result.is_ok(), "compiler actor should exit after shutdown");
    }
}
