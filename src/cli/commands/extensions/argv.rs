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
    context.hook = CallContext::read()?.and_then(|parent| parent.hook);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::cli::external::{CommandExit, run as run_process};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tola_build::report::ReportSources;

    fn report_channel(severity: &str) -> ReportChannel {
        let reports = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
        std::fs::write(
            reports.path(),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "diagnostics": [{"severity": severity, "message": "catalog issue", "code": "rule"}],
            }))
            .unwrap(),
        )
        .unwrap();
        reports
    }

    #[test]
    fn report_severity_controls_success() {
        let out = super::super::super::testing::buffered_output();
        complete_report(Ok(()), report_channel("warning"), &out).unwrap();
        let err = complete_report(Ok(()), report_channel("error"), &out).unwrap_err();
        let diags = tola_diagnostic::attached(&err).unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[0].producer.as_ref().unwrap().name.as_deref(),
            Some("command:catalog.scan"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_survives_reports() {
        use std::os::unix::process::ExitStatusExt;

        let out = super::super::super::testing::buffered_output();
        for invalid in [false, true] {
            let reports = report_channel("warning");
            if invalid {
                std::fs::write(reports.path(), b"partial report").unwrap();
            }
            let done = Err(CommandExit(std::process::ExitStatus::from_raw(37 << 8)).into());
            let err = complete_report(done, reports, &out).unwrap_err();
            assert_eq!(
                err.downcast_ref::<CommandExit>().unwrap().0.code(),
                Some(37)
            );
            let diags = tola_diagnostic::attached(&err).unwrap();
            assert_eq!(diags.len(), 1);
            assert_eq!(
                diags[0].severity,
                if invalid {
                    Severity::Error
                } else {
                    Severity::Warning
                }
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn expanded_argv_reaches_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mod_dir = root.join("module with spaces");
        std::fs::create_dir(&mod_dir).unwrap();
        let mod_cmds = ModuleCommands {
            name: "notes".into(),
            root: mod_dir.clone(),
            manifest: PathBuf::from("/module with spaces/tola-module.toml"),
            commands: BTreeMap::new(),
            initializer: None,
            options: Default::default(),
        };
        let record = root.join("arguments.txt");
        let script = mod_dir.join("record.sh");
        std::fs::write(
            &script,
            "record=\"$1\"\nshift\nprintf '%s\\n' \"$@\" > \"$record\"\nexit 37\n",
        )
        .unwrap();
        let argv = vec![
            "sh".into(),
            "{module}/record.sh".into(),
            "{project}/arguments.txt".into(),
        ];
        let reports = ReportChannel::new("command:notes.record", ReportSources::new()).unwrap();
        let cmd = process_command(
            &argv,
            &["a title".into(), "--".into(), "$(literal)".into()],
            Site {
                root: root.to_owned(),
                config_path: root.join("tola.toml"),
                content_root: root.join("content"),
            },
            &mod_cmds,
            InputScope::Offline,
            &reports,
        )
        .unwrap();
        let err = run_process(cmd, &crate::cancellation::Cancellation::default()).unwrap_err();
        assert_eq!(
            err.downcast_ref::<CommandExit>().unwrap().0.code(),
            Some(37)
        );
        let recorded = std::fs::read_to_string(&record).unwrap();
        assert_eq!(
            recorded.lines().collect::<Vec<_>>(),
            ["a title", "--", "$(literal)"]
        );
    }
}
