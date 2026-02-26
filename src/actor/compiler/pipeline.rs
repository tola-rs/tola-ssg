use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::address::{GLOBAL_ADDRESS_SPACE, PermalinkUpdate};
use crate::reload::compile::CompileOutcome;

use super::CompilerActor;
use super::tasks::compile_batch;

impl CompilerActor {
    /// Compile a single file (blocking).
    pub(super) async fn compile_one(&mut self, path: &Path) {
        let config = Arc::clone(&self.config);
        let path = path.to_path_buf();

        let result = tokio::task::spawn_blocking(move || {
            let outcome = crate::reload::compile::compile_page(&path, &config);
            // spawn_blocking threads are not rayon workers.
            crate::compiler::dependency::flush_current_thread_deps();
            outcome
        })
        .await;

        match result {
            Ok(outcome) => self.route(outcome).await,
            Err(e) => crate::log!("compile"; "error: {}", e),
        }
    }

    /// Compile multiple files in parallel (blocking).
    pub(super) async fn compile_batch_blocking(&mut self, paths: Vec<PathBuf>) {
        let outcomes = compile_batch(paths, Arc::clone(&self.config)).await;
        for outcome in outcomes {
            self.route(outcome).await;
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
    pub(super) async fn route(&mut self, outcome: CompileOutcome) {
        let msg = match outcome {
            CompileOutcome::Vdom {
                path,
                url_path,
                vdom,
                warnings,
            } => {
                let permalink_change = update_address_space(&path, &url_path);
                crate::actor::messages::VdomMsg::Process {
                    path,
                    url_path,
                    vdom,
                    permalink_change,
                    warnings,
                }
            }
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

fn update_address_space(path: &Path, url_path: &crate::core::UrlPath) -> Option<PermalinkUpdate> {
    let mut space = GLOBAL_ADDRESS_SPACE.write();
    let update = space.update_source_url(path, url_path);
    crate::debug!(
        "permalink";
        "update({}, {}) = {:?}",
        path.display(),
        url_path,
        update
    );
    match update {
        PermalinkUpdate::Unchanged => None,
        _ => Some(update),
    }
}
