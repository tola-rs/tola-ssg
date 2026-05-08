use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::address::{GLOBAL_ADDRESS_SPACE, PermalinkUpdate};
use crate::page::PageRoute;
use crate::reload::compile::CompileOutcome;

use super::CompilerActor;
use super::tasks::compile_batch;

impl CompilerActor {
    /// Compile a single file (blocking).
    pub(super) async fn compile_one(&mut self, path: &Path) {
        let config = Arc::clone(&self.config);
        let store = Arc::clone(&self.store);
        let path = path.to_path_buf();
        crate::compiler::scheduler::SCHEDULER.invalidate(&path);

        let result = tokio::task::spawn_blocking(move || {
            let outcome = crate::reload::compile::compile_page(&path, &config, &store);
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
        let outcomes =
            compile_batch(paths, Arc::clone(&self.config), Arc::clone(&self.store)).await;
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
                route,
                title,
                url_path,
                vdom,
                warnings,
            } => {
                let permalink_change = update_address_space(route, title);
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

fn update_address_space(route: PageRoute, title: Option<String>) -> Option<PermalinkUpdate> {
    let path = route.source.clone();
    let url_path = route.permalink.clone();
    let mut space = GLOBAL_ADDRESS_SPACE.write();
    let update = space.update_page_checked(route, title);
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

#[cfg(test)]
mod tests {
    use super::update_address_space;
    use crate::address::{GLOBAL_ADDRESS_SPACE, PermalinkUpdate};
    use crate::core::UrlPath;
    use crate::page::PageRoute;
    use std::path::PathBuf;

    fn test_route(source: &str, permalink: &str, output: &str) -> PageRoute {
        PageRoute {
            source: PathBuf::from(source),
            permalink: UrlPath::from_page(permalink),
            output_file: PathBuf::from(output),
            is_index: false,
            is_404: false,
            output_dir: PathBuf::new(),
            full_url: String::new(),
        }
    }

    #[test]
    fn update_address_space_reports_permalink_change_for_existing_source_mapping() {
        let old_url = UrlPath::from_page("/old/");

        {
            let mut space = GLOBAL_ADDRESS_SPACE.write();
            space.clear();
            space.register_page(
                test_route("content/post.typ", "/old/", "public/old/index.html"),
                Some("Post".to_string()),
            );
        }

        let change = update_address_space(
            test_route("content/post.typ", "/new/", "public/new/index.html"),
            None,
        );

        match change {
            Some(PermalinkUpdate::Changed { old_url: old }) => {
                assert_eq!(old, old_url);
            }
            other => panic!("expected permalink change, got {:?}", other),
        }

        let mut space = GLOBAL_ADDRESS_SPACE.write();
        space.clear();
    }
}
