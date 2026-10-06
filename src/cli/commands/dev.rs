use crate::cancellation::Cancellation;
use anyhow::Result;

use crate::cli::output::CommandOutput;
use crate::cli::{BuildOverrideArgs, ConfigFileArgs, DevelopmentArgs, TypstPackageArgs};
use crate::config::ConfigOverrides;
use tola_build::InputScope;

#[allow(clippy::too_many_arguments)]
pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    build: BuildOverrideArgs,
    development: DevelopmentArgs,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        server: crate::cli::config::server_overrides(&development.server),
        watch: development.watch,
    };
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let (config, server, dev, loader) = loaded.into_parts();
    crate::dev::run(
        config,
        server,
        dev,
        loader,
        resources,
        output.clone(),
        cancellation.clone(),
    )
}
