//! `tola check`: build and validate the candidate without publishing it.
//!
//! The command never writes `build.publish-dir`, and hooks run unless the caller skips them.
//! `--no-build` stops at parsing each source the site owns.
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tola_build::has_typ_extension;
use tola_diagnostic::DiagnosticError;

pub(in crate::cli) fn run(
    request: super::SiteRequest<'_>,
    json: bool,
    no_hooks: bool,
) -> Result<()> {
    let mut report = super::report::SiteReport::new("check", request.scope, "candidate", !no_hooks);
    let output = request.output;
    let cancellation = request.cancellation;
    let completion = execute(request, json, no_hooks, &mut report)
        .and_then(|()| cancellation.token().ensure_active().map_err(Into::into));
    report.finish(completion, output, json)
}

fn execute(
    request: super::SiteRequest<'_>,
    json: bool,
    no_hooks: bool,
    report: &mut super::report::SiteReport,
) -> Result<()> {
    let output = request.output;
    let cancellation = request.cancellation;
    cancellation.token().ensure_active()?;
    let overrides = crate::cli::config::config_overrides(&request.build);
    let started = Instant::now();
    let config = Arc::new(request.load_config(&overrides)?.into_config());
    report.configuration(&config);
    output.status(if no_hooks {
        "Checking site without hooks…"
    } else {
        "Checking site…"
    })?;
    if !no_hooks {
        output.hook_overview(
            &config.build().hooks,
            tola_build::build::BuildMode::Production,
            crate::terminal::PRE_PUBLICATION_STAGES,
        )?;
    }
    let mut session = tola_build::BuildSession::with_resources(request.resources);
    let mut build = tola_build::build::BuildRequest::default();
    build.cancellation = cancellation.token();
    build.capture_report = json;
    build.hook_execution = if no_hooks {
        tola_build::build::HookExecution::Skip
    } else {
        tola_build::build::HookExecution::Run
    };
    let locked = tola_build::build::SiteBuildGuard::build(&mut session, config, build, || {
        let _ = output.waiting_for_build();
    })
    .map_err(|failure| {
        report.failed_candidate(&failure);
        failure.into_error()
    })?;
    report.candidate(locked.site());
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

/// Parse every source the site owns, compiling nothing.
///
/// The walk is [`tola_build::filesystem::owned_files`], so this analysis reaches exactly the files
/// `tola fmt` would rewrite. What a source *means* — values, routes, references, output — is the
/// build's to establish, not this.
pub(in crate::cli) fn sources(request: &super::SiteRequest<'_>, json: bool) -> Result<()> {
    let mut report = super::report::SiteReport::new("check", request.scope, "source", false);
    let completion = (|| -> Result<()> {
        request.cancellation.token().ensure_active()?;
        let overrides = crate::cli::config::config_overrides(&request.build);
        let started = Instant::now();
        let config = request.load_config(&overrides)?.into_config();
        report.configuration(&config);
        request.output.status("Reading site sources…")?;
        let root = config.get_root();
        let files = tola_build::filesystem::owned_files(&config, &request.cancellation.token())?;
        let mut diagnostics = Vec::new();
        let mut examined = 0;
        for source in files.iter().filter(|path| has_typ_extension(path)) {
            request.cancellation.token().ensure_active()?;
            let Ok(bytes) = std::fs::read(source) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                diagnostics.push(
                    tola_diagnostic::unreadable_source(
                        crate::codes::check::UNREADABLE,
                        &tola_build::filesystem::display_path(source, root),
                    )
                    .with_help("Save the file as UTF-8, then run `tola check` again"),
                );
                continue;
            };
            examined += 1;
            if let Some(range) = tola_typst_syntax::errors::first_error_range(text) {
                diagnostics.push(tola_diagnostic::unparsable_source(
                    crate::codes::check::SYNTAX,
                    &tola_build::filesystem::display_path(source, root),
                    text,
                    Some(range.start),
                ));
            }
        }
        if super::holds_error(&diagnostics) {
            return Err(DiagnosticError::new("source analysis failed", diagnostics).into());
        }
        request.output.summary(crate::terminal::site_summary(
            "Checked",
            started.elapsed(),
            &crate::terminal::plural_count(examined, "source"),
        ))?;
        Ok(())
    })()
    .and_then(|()| {
        request
            .cancellation
            .token()
            .ensure_active()
            .map_err(Into::into)
    });
    report.finish(completion, request.output, json)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{buffered_output, site};
    use super::*;
    use crate::cancellation::Cancellation;
    use crate::cli::output::CommandOutput;
    use crate::cli::{BuildOverrideArgs, ConfigFileArgs, TypstPackageArgs};
    use tola_build::InputScope;

    fn analyse(config_file: ConfigFileArgs, output: &CommandOutput) -> Result<()> {
        let cancellation = Cancellation::default();
        let request = super::super::SiteRequest::new(
            config_file,
            TypstPackageArgs::default(),
            InputScope::Online,
            BuildOverrideArgs::default(),
            crate::cli::CompilerFontArgs::default(),
            output,
            &cancellation,
        )
        .unwrap();
        sources(&request, false)
    }

    #[test]
    fn broken_source_reported_place() {
        let (directory, config) = site(&[("content/broken.typ", "= Title\n#let = \n")]);
        let output = buffered_output();

        let error = analyse(config, &output).unwrap_err();

        let diagnostics = tola_diagnostic::attached(&error).expect("the failure is stated");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, crate::codes::check::SYNTAX);
        let location = diagnostics[0].location.as_ref().expect("a location");
        assert_eq!(location.path, "content/broken.typ");
        assert!(
            location.line.is_some(),
            "the failure is placed: {location:?}"
        );
        assert!(
            !directory.path().join("public").exists(),
            "nothing is built"
        );
    }

    #[test]
    fn readable_sources_pass_and_write_nothing() {
        let (directory, config) = site(&[("content/index.typ", "#let  x =  1\n")]);
        let output = buffered_output();

        analyse(config, &output).unwrap();

        assert_eq!(
            std::fs::read_to_string(directory.path().join("content/index.typ")).unwrap(),
            "#let  x =  1\n"
        );
        assert!(
            !directory.path().join("public").exists(),
            "nothing is built"
        );
    }

    #[test]
    fn state_site_sources_not_analysed() {
        let (_directory, config) = site(&[
            ("public/broken.typ", "#let = \n"),
            (".tola/broken.typ", "#let = \n"),
        ]);
        let output = buffered_output();

        analyse(config, &output).unwrap();
    }

    #[test]
    fn non_utf8_source_is_reported() {
        let (directory, config) = site(&[]);
        std::fs::write(directory.path().join("content/binary.typ"), [0xff, 0xfe]).unwrap();
        let output = buffered_output();

        let error = analyse(config, &output).unwrap_err();

        let diagnostics = tola_diagnostic::attached(&error).expect("the failure is stated");
        assert_eq!(diagnostics[0].code, crate::codes::check::UNREADABLE);
        let location = diagnostics[0].location.as_ref().expect("a location");
        assert_eq!(location.path, "content/binary.typ");
    }
}
