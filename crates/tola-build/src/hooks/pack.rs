//! External author hooks, without a configured site or module instance.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use tola_diagnostic::Diagnostic;
use tola_subprocess::Command;

use crate::cancellation::BuildCancellation;
use crate::context::{Context, Hook};
use crate::report::{ReportChannel, ReportSources};

use super::command::{finish_report, run_observed};

/// Run a module's external before-pack action in the author source directory.
pub fn run_before_pack(
    name: &str,
    argv: &[String],
    source: &Path,
    temp: &Path,
    cancellation: &BuildCancellation,
) -> Result<Vec<Diagnostic>> {
    cancellation.ensure_active()?;
    let executable =
        std::env::current_exe().context("could not locate the running Tola executable")?;
    let bindings = [
        ("{tola}", executable.as_os_str()),
        ("{module}", source.as_os_str()),
    ];
    let arguments = argv
        .iter()
        .map(|argument| {
            for marker in ["{project}", "{name}"] {
                ensure!(
                    !argument.contains(marker),
                    "before-pack hook `{name}` cannot use `{marker}`"
                );
            }
            Ok(tola_module::expand_argument(argument, &bindings))
        })
        .collect::<Result<Vec<_>>>()?;
    let reports = ReportChannel::new_in(
        temp,
        format!("hook:before-pack:{name}"),
        ReportSources::new(),
    )?;
    let mut context = before_pack_context(executable, name, source, temp);
    context.report_path = Some(reports.path().to_owned());
    let command = context.command(
        Command::from_args(arguments)
            .cwd(source)
            .env("TMPDIR", temp)
            .env("TMP", temp)
            .env("TEMP", temp),
        reports.directory(),
    )?;
    let completion = run_observed(&command, name, &|| cancellation.is_cancelled());
    let mut diagnostics = Vec::new();
    let hook = format!("the `{name}` hook in `hooks.before-pack`");
    finish_report(
        &hook,
        reports,
        completion,
        Some(cancellation),
        &mut diagnostics,
    )?;
    Ok(diagnostics)
}

/// Describe a before-pack invocation for either execution mode.
pub fn before_pack_context(
    executable: std::path::PathBuf,
    name: &str,
    source: &Path,
    temp: &Path,
) -> Context {
    let mut context = Context::new(executable, None);
    context.hook = Some(Hook {
        name: name.to_owned(),
        stage: "before-pack".into(),
        mode: None,
        input_dir: Some(source.to_owned()),
        output_dir: None,
        outputs: Vec::new(),
        cache_dir: None,
        temp_dir: temp.to_owned(),
    });
    context
}
