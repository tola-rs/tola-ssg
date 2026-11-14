//! Declared module commands that run an external program.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{Result, bail, ensure};
use tola_build::context::{Context as CallContext, Module, Site};
use tola_build::report::ReportChannel;
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_input::InputScope;
use tola_module::ModuleCommands;
use tola_subprocess::Command;

use crate::cli::output::CommandOutput;

fn expand(argument: &str, project: &Path, module: &ModuleCommands, executable: &Path) -> OsString {
    tola_module::expand_argument(
        argument,
        &[
            ("{project}", project.as_os_str()),
            ("{module}", module.root.as_os_str()),
            ("{name}", std::ffi::OsStr::new(&module.name)),
            ("{tola}", executable.as_os_str()),
        ],
    )
}

pub(super) fn process_command(
    argv: &[String],
    tail: &[OsString],
    site: Site,
    module: &ModuleCommands,
    scope: InputScope,
    reports: &ReportChannel,
) -> Result<Command> {
    let project = &site.root;
    let Some(program) = argv.first() else {
        bail!("declared command has no executable");
    };
    let executable = crate::cli::external::current_executable()?;
    let program = expand(program, project, module, &executable);
    ensure!(
        !program.is_empty(),
        "declared command has an empty executable"
    );
    let command = Command::new(program)
        .args(
            argv[1..]
                .iter()
                .map(|arg| expand(arg, project, module, &executable)),
        )
        .args(tail.iter().cloned())
        .cwd(project);
    let mut context = CallContext::new(executable, Some(scope));
    context.site = Some(site);
    context.module = Some(Module {
        name: module.name.clone(),
        root: module.root.clone(),
        manifest: module.manifest.clone(),
        options: Some(serde_json::to_value(&module.options)?),
    });
    context.report_path = Some(reports.path().to_owned());
    context.command(command, reports.directory())
}

pub(super) fn complete_report(
    completion: Result<()>,
    reports: ReportChannel,
    output: &CommandOutput,
) -> Result<()> {
    let diagnostics = match reports.read() {
        Ok(diagnostics) => diagnostics.unwrap_or_default(),
        Err(error) => {
            let message = "invalid module command diagnostic report";
            let diagnostic =
                Diagnostic::new(crate::codes::command::FAILED, Severity::Error, message)
                    .with_help("Ask the module author to check the report format and use only source locations supplied by Tola");
            let error = anyhow::Error::new(error).context(message);
            return match completion {
                Ok(()) => Err(error.context(DiagnosticError::new(message, vec![diagnostic]))),
                Err(process_error) => Err(process_error
                    .context(error)
                    .context(DiagnosticError::new(message, vec![diagnostic]))),
            };
        }
    };
    if let Err(error) = completion {
        return if diagnostics.is_empty() {
            Err(error)
        } else {
            let message = error.to_string();
            Err(error.context(DiagnosticError::new(message, diagnostics)))
        };
    }
    if super::super::holds_error(&diagnostics) {
        return Err(DiagnosticError::new(
            "a reported error prevents this module command from succeeding",
            diagnostics,
        )
        .into());
    }
    output.diagnostics(&diagnostics)?;
    Ok(())
}
