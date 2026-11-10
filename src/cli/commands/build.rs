//! `tola build`: one production-mode build, published whole, then its after-publish commands.
//!
//! The build library validates the candidate; choosing to publish it, and reporting what happened
//! afterwards, is this command's policy.
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

pub(in crate::cli) fn run(request: super::SiteRequest<'_>, json: bool, pretty: bool) -> Result<()> {
    let mut report = super::report::SiteReport::new("build", request.scope(), "candidate", true);
    let output = request.output;
    let cancellation = request.cancellation;
    let completion = execute(request, json, &mut report)
        .and_then(|()| cancellation.token().ensure_active().map_err(Into::into));
    report.finish(completion, output, json, pretty)
}

fn execute(
    request: super::SiteRequest<'_>,
    json: bool,
    report: &mut super::report::SiteReport,
) -> Result<()> {
    let output = request.output;
    let cancellation = request.cancellation;
    cancellation.token().ensure_active()?;
    let overrides = crate::cli::config::config_overrides(&request.build);
    let config = Arc::new(request.load_config(&overrides)?.into_config());
    report.configuration(&config);
    output.status("Building site…")?;
    output.hook_overview(
        config.hooks(),
        tola_build::build::BuildMode::Production,
        crate::terminal::EVERY_STAGE,
    )?;
    let started = Instant::now();
    let mut session = tola_build::BuildSession::with_resources(request.resources);
    let mut build = tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Production);
    build.cancellation = cancellation.token();
    build.capture_report = json;
    let locked = tola_build::build::SiteBuildGuard::build_for_publication(
        &mut session,
        config,
        build,
        || {
            let _ = output.waiting_for_build();
        },
    )
    .map_err(|failure| {
        report.failed_candidate(&failure);
        anyhow::Error::from(failure)
    })?;
    report.candidate_report(locked.site());
    let outcome = locked.write_site()?;
    report.diagnostics(outcome.diagnostics());
    report.published = true;
    drop(locked);
    output.diagnostics(outcome.diagnostics())?;
    let counts = outcome.counts();
    output.summary(crate::terminal::site_summary(
        "Built",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    ))?;
    match outcome.run_after_publish(&cancellation.token()) {
        Ok(diagnostics) => {
            output.diagnostics(&diagnostics)?;
            report.diagnostics(&diagnostics);
        }
        Err(error) => {
            let error = error
                .context("site output was written, but an after-publish command did not finish");
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
            return Err(tola_diagnostic::DiagnosticError::attach(error, diagnostics).into());
        }
    }
    Ok(())
}
