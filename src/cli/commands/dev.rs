//! `tola dev`: resolve configuration, then hand it to the development runtime.
//!
//! The rebuild loop, watch boundaries, revision installation, and browser reload belong to
//! `crate::dev`; this command adds the invocation's overrides and starts them, choosing no build
//! semantics.
use anyhow::Result;

use crate::cli::DevelopmentArgs;
use crate::config::ConfigOverrides;

pub(in crate::cli) fn run(
    request: super::SiteRequest<'_>,
    development: DevelopmentArgs,
) -> Result<()> {
    let overrides = ConfigOverrides {
        server: crate::cli::config::server_overrides(&development.server),
        watch: development.watch,
        ..crate::cli::config::config_overrides(&request.build)
    };
    let (config, server, dev, config_loader) = request.load_config(&overrides)?.into_parts();
    crate::dev::run(
        crate::dev::ServerLaunch {
            config,
            server,
            config_loader,
            resources: request.resources,
            output: request.output.clone(),
            shutdown: request.cancellation.clone(),
            events: crate::dev::events::Events(development.json),
        },
        dev,
        development.no_reuse,
    )
}
