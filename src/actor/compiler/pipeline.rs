use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::reload::compile::CompileOutcome;

use super::CompilerActor;
use super::tasks::compile_batch;

impl CompilerActor {
    /// Compile a single file (blocking).
    pub(super) async fn compile_one(&mut self, path: &Path) {
        let config = self.config.current();
        let compile_config = Arc::clone(&config);
        let state = Arc::clone(&self.state);
        let path = path.to_path_buf();
        crate::compiler::scheduler::SCHEDULER.invalidate(&path);

        let result = tokio::task::spawn_blocking(move || {
            let outcome = crate::reload::compile::compile_page(&path, &compile_config, &state);
            // spawn_blocking threads are not rayon workers.
            crate::compiler::dependency::flush_current_thread_deps();
            outcome
        })
        .await;

        match result {
            Ok(outcome) => self.route(outcome, config).await,
            Err(e) => crate::log!("compile"; "error: {}", e),
        }
    }

    /// Compile multiple files in parallel (blocking).
    pub(super) async fn compile_batch_blocking(&mut self, paths: Vec<PathBuf>) {
        let config = self.config.current();
        let outcomes = compile_batch(paths, Arc::clone(&config), Arc::clone(&self.state)).await;
        for outcome in outcomes {
            self.route(outcome, Arc::clone(&config)).await;
        }
    }

    /// Recompile pages using @tola/* virtual packages.
    pub(super) async fn recompile_virtual_users(&mut self) {
        use crate::compiler::dependency::collect_virtual_dependents;

        let all_dependents = collect_virtual_dependents();

        if !all_dependents.is_empty() {
            crate::debug!(
                "compile";
                "recompiling {} virtual package users",
                all_dependents.len()
            );
            self.compile_batch_blocking(all_dependents.into_iter().collect())
                .await;
        } else {
            crate::debug!("compile"; "no virtual package users to recompile");
        }
    }

    /// Route compilation outcome to VdomActor.
    pub(super) async fn route(
        &mut self,
        outcome: CompileOutcome,
        config: Arc<crate::config::SiteConfig>,
    ) {
        let msg = match outcome {
            CompileOutcome::Vdom {
                path,
                url_path,
                vdom,
                permalink_change,
                warnings,
            } => crate::actor::messages::VdomMsg::Process {
                config,
                path,
                url_path,
                vdom,
                permalink_change,
                warnings,
            },
            CompileOutcome::Reload { reason } => crate::actor::messages::VdomMsg::Reload { reason },
            CompileOutcome::Skipped => crate::actor::messages::VdomMsg::Skip,
            CompileOutcome::Error {
                path,
                url_path,
                error,
            } => crate::actor::messages::VdomMsg::Error {
                path,
                url_path: url_path.unwrap_or_default(),
                error,
            },
        };
        let _ = self.vdom_tx.send(msg).await;
    }
}
