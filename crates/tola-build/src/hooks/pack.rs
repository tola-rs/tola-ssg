//! External author hooks, without a configured site or module instance.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use tola_diagnostic::Diagnostic;
use tola_subprocess::Command;

use crate::cancellation::BuildCancellation;
use crate::report::{REPORT_PATH_ENV, ReportChannel, ReportSources};

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
    let mut command = Command::from_args(arguments).cwd(source);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("TOLA_PROJECT_")
            || name.starts_with("TOLA_MODULE_")
            || name.starts_with("TOLA_HOOK_")
            || name.starts_with("TOLA_INIT_")
        {
            command = command.env_remove(key);
        }
    }
    let command = command
        .env_remove("TOLA_CONFIG_PATH")
        .env_remove("TOLA_CONTENT_ROOT")
        .env_remove("TOLA_BUILD_MODE")
        .env_remove("TOLA_INPUT_SCOPE")
        .env("TOLA_EXECUTABLE", executable)
        .env("TOLA_HOOK_STAGE", "before-pack")
        .env("TOLA_HOOK_NAME", name)
        .env("TOLA_HOOK_INPUT_DIR", source)
        .env("TOLA_HOOK_TEMP_DIR", temp)
        .env("TMPDIR", temp)
        .env("TMP", temp)
        .env("TEMP", temp)
        .env(REPORT_PATH_ENV, reports.path());
    let completion = run_observed(&command, name, &|| cancellation.is_cancelled());
    let mut diagnostics = Vec::new();
    finish_report(reports, completion, Some(cancellation), &mut diagnostics)?;
    Ok(diagnostics)
}
