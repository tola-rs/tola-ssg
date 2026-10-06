use crate::cancellation::Cancellation;
use std::sync::Arc;
use std::time::Instant;

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
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &overrides,
        output,
        cancellation,
    )?;
    let config = Arc::new(loaded.into_config());
    output.status("Building site…")?;
    output.hook_overview(
        &config.build().hooks,
        tola_build::build::BuildMode::Production,
        crate::terminal::EVERY_STAGE,
    )?;
    let started = Instant::now();
    let mut session = tola_build::BuildSession::with_resources(resources);
    let mut request =
        tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production);
    request.cancellation = cancellation.token();
    let locked = tola_build::build::SiteBuildGuard::build_for_publication(
        &mut session,
        config,
        request,
        || {
            let _ = output.waiting_for_build();
        },
    )
    .map_err(|failure| failure.into_error())?;
    let outcome = locked.write_site()?;
    drop(locked);
    output.diagnostics(outcome.diagnostics())?;
    let counts = outcome.counts();
    output.summary(crate::terminal::site_summary(
        "Built",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    ))?;
    if let Err(error) = outcome.run_after_publish(&cancellation.token()) {
        let error =
            error.context("site output was written, but an after-publish command did not finish");
        if error
            .chain()
            .any(|cause| cause.is::<tola_build::cancellation::BuildCancelled>())
        {
            return Err(error);
        }
        // The rendered diagnostic belongs to the failing hook; the committed output is
        // stated here because only this boundary knows the write already happened.
        let diagnostics =
            crate::cli::output::attached_or_fallback(&error, crate::codes::hook::AFTER_PUBLISH)
                .into_iter()
                .map(|diagnostic| diagnostic.with_note("the site output was written"))
                .collect();
        return Err(tola_build::diagnostic::DiagnosticError::attach(error, diagnostics).into());
    }
    Ok(())
}
