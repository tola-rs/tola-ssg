use anyhow::Result;

use crate::cli::ServerArgs;
use crate::config::ConfigOverrides;

pub(in crate::cli) fn run(request: super::SiteRequest<'_>, server: ServerArgs) -> Result<()> {
    let super::SiteRequest {
        config_file,
        packages,
        scope,
        build,
        resources,
        output,
        cancellation,
    } = request;
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        server: crate::cli::config::server_overrides(&server),
        ..ConfigOverrides::default()
    };
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let (config, server, _, loader) = loaded.into_parts();
    crate::dev::run_preview(crate::dev::ServerLaunch {
        config,
        server,
        config_loader: loader,
        resources,
        output: output.clone(),
        shutdown: cancellation.clone(),
    })
}
