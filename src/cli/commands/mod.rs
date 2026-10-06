pub(super) mod build;
pub(super) mod check;
pub(super) mod completions;
pub(super) mod config;
pub(super) mod dev;
pub(super) mod doctor;
pub(super) mod editor;
pub(super) mod help;
pub(super) mod init;
pub(super) mod inspect;
pub(super) mod lsp;
pub(super) mod manpage;
pub(super) mod preview;
pub(super) mod skill;
pub(super) mod vendor;

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;

/// Build one complete production candidate while holding the cross-process site build lock.
///
/// The returned guard keeps the lock: a publishing caller writes through it, and a
/// read-only caller checks freshness and releases it.
pub(in crate::cli) fn build_candidate(
    config: std::sync::Arc<tola_build::config::ResolvedSiteConfig>,
    cancellation: &Cancellation,
    hook_execution: tola_build::build::HookExecution,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
) -> anyhow::Result<tola_build::build::SiteBuildGuard> {
    let mut session = tola_build::BuildSession::with_resources(resources);
    let mut request =
        tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production);
    request.cancellation = cancellation.token();
    request.hook_execution = hook_execution;
    tola_build::build::SiteBuildGuard::build(&mut session, config, request, || {
        let _ = output.waiting_for_build();
    })
    .map_err(|failure| failure.into_error())
}
