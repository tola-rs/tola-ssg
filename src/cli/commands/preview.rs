//! `tola preview`: one production-mode build, served locally instead of published.
//!
//! It runs the pre-publication hook stages only, so a preview never writes `build.publish-dir` and
//! never runs an `after-publish` command.
use anyhow::Result;

use crate::cli::ServerArgs;
use crate::config::ConfigOverrides;

pub(in crate::cli) fn run(request: super::SiteRequest<'_>, server: ServerArgs) -> Result<()> {
    let overrides = ConfigOverrides {
        server: crate::cli::config::server_overrides(&server),
        ..crate::cli::config::config_overrides(&request.build)
    };
    let (config, server, _, config_loader) = request.load_config(&overrides)?.into_parts();
    crate::dev::run_preview(crate::dev::ServerLaunch {
        config,
        server,
        config_loader,
        resources: request.resources,
        output: request.output.clone(),
        shutdown: request.cancellation.clone(),
        events: crate::dev::events::Events(false),
    })
}
