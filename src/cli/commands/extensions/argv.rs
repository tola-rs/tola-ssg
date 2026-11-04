//! Declared module commands that run an external program.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{Result, bail, ensure};
use tola_build::InputScope;
use tola_build::modules::ModuleCommands;
use tola_build::report::ReportChannel;
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_subprocess::Command;

use crate::cli::output::CommandOutput;

pub(super) fn expand(argument: &str, project: &Path, module: &Path, executable: &Path) -> OsString {
    tola_build::modules::expand_argument(
        argument,
        &[
            ("{project}", project.as_os_str()),
            ("{module}", module.as_os_str()),
            ("{tola}", executable.as_os_str()),
        ],
    )
}

pub(super) fn process_command(
    argv: &[String],
    tail: &[OsString],
    project: &Path,
    config: &Path,
    module: &ModuleCommands,
    scope: InputScope,
) -> Result<Command> {
    let Some(program) = argv.first() else {
        bail!("declared command has no executable");
    };
    let executable = crate::cli::external::current_executable()?;
    let program = expand(program, project, &module.root, &executable);
    ensure!(
        !program.is_empty(),
        "declared command has an empty executable"
    );
    let command = Command::new(program)
        .args(
            argv[1..]
                .iter()
                .map(|arg| expand(arg, project, &module.root, &executable)),
        )
        .args(tail.iter().cloned())
        .cwd(project)
        .env("TOLA_PROJECT_ROOT", project.as_os_str())
        .env("TOLA_MODULE_ROOT", module.root.as_os_str())
        .env("TOLA_MODULE_NAME", module.name.as_str())
        .env("TOLA_MODULE_MANIFEST", module.manifest.as_os_str())
        .env_remove("TOLA_MODULE_OPTIONS")
        .env("TOLA_CONFIG_PATH", config.as_os_str())
        .env("TOLA_INPUT_SCOPE", scope.as_str())
        .env("TOLA_EXECUTABLE", executable);
    Ok(command)
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

