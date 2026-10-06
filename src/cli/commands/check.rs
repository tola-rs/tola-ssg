use crate::cancellation::Cancellation;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::output::CommandOutput;
use crate::cli::{BuildOverrideArgs, ConfigFileArgs, TypstPackageArgs};
use crate::config::ConfigOverrides;
use tola_build::InputScope;

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    build: BuildOverrideArgs,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let overrides = ConfigOverrides {
        build: crate::cli::config::build_overrides(&build),
        ..ConfigOverrides::default()
    };
    let started = std::time::Instant::now();
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let config = Arc::new(loaded.into_config());
    output.status("Checking site…")?;
    output.hook_overview(
        &config.build().hooks,
        tola_build::build::BuildMode::Production,
        crate::terminal::PRE_PUBLICATION_STAGES,
    )?;
    let locked = super::build_candidate(
        config,
        cancellation,
        tola_build::build::HookExecution::Run,
        resources,
        output,
    )?;
    locked.ensure_fresh(&cancellation.token())?;
    let site = locked.release();
    output.diagnostics(site.diagnostics())?;
    let counts = site.graph().counts();
    output.summary(crate::terminal::site_summary(
        "Checked",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    ))?;
    Ok(())
}
