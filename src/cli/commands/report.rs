//! The result of one CLI operation, including stages completed before an error.

use anyhow::Result;
use serde::Serialize;
use tola_build::build::{BuildFailure, BuildReport, SiteBuild};
use tola_diagnostic::Diagnostic;
use tola_input::InputScope;

use crate::cli::output::CommandOutput;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::cli) struct SiteReport {
    format_version: u32,
    operation: &'static str,
    success: bool,
    cancelled: bool,
    pub published: bool,
    input: ReportInput,
    diagnostics: Vec<Diagnostic>,
    #[serde(flatten)]
    build: BuildReport,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReportInput {
    mode: &'static str,
    phase: &'static str,
    scope: &'static str,
    hook_execution: &'static str,
}

impl SiteReport {
    pub fn new(
        operation: &'static str,
        scope: InputScope,
        phase: &'static str,
        run_hooks: bool,
    ) -> Self {
        Self {
            format_version: 1,
            operation,
            success: false,
            cancelled: false,
            published: false,
            input: ReportInput {
                mode: "disk",
                phase,
                scope: scope.as_str(),
                hook_execution: if run_hooks { "run" } else { "skipped" },
            },
            diagnostics: Vec::new(),
            build: BuildReport::default(),
        }
    }

    pub fn configuration(&mut self, config: &tola_build::config::ResolvedSiteConfig) {
        self.diagnostics
            .extend(tola_build::config::diagnostic::warning_diagnostics(config));
    }

    pub fn candidate(&mut self, site: &SiteBuild) {
        self.candidate_report(site);
        self.diagnostics.extend_from_slice(site.diagnostics());
    }

    pub fn candidate_report(&mut self, site: &SiteBuild) {
        self.build = site.report().cloned().unwrap_or_default();
    }

    pub fn diagnostics(&mut self, diagnostics: &[Diagnostic]) {
        self.diagnostics.extend_from_slice(diagnostics);
    }

    pub fn failed_candidate(&mut self, failure: &BuildFailure) {
        self.build = failure.report().cloned().unwrap_or_default();
    }

    pub fn finish(
        mut self,
        completion: Result<()>,
        output: &CommandOutput,
        json: bool,
        pretty: bool,
    ) -> Result<()> {
        self.success = completion.is_ok();
        if let Err(error) = &completion {
            self.cancelled = tola_build::cancellation::is_cancelled(error);
            if !self.cancelled {
                self.diagnostics
                    .extend(crate::cli::output::attached_or_fallback(
                        error,
                        tola_diagnostic::codes::build::SITE,
                    ));
            }
        }
        let written = if json {
            let encoded = if pretty {
                serde_json::to_string_pretty(&self)
            } else {
                serde_json::to_string(&self)
            };
            encoded
                .map_err(anyhow::Error::from)
                .and_then(|encoded| output.write_stdout_line(encoded))
        } else {
            Ok(())
        };
        match (completion, written) {
            (Err(error), Err(write_error)) => Err(error.context(write_error)),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}
