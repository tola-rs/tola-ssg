//! Standalone module author commands.

use std::path::Path;

use anyhow::Result;
use tola_build::report::ReportSources;
use tola_diagnostic::DiagnosticError;
use tola_module::{Action, check, pack};
use tola_typst::BundleCancellation;

use crate::cancellation::Cancellation;
use crate::cli::args::ModuleCommand;
use crate::cli::output::CommandOutput;
use crate::terminal::display_path;

pub(in crate::cli) fn run(
    command: ModuleCommand,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let token = cancellation.token();
    match command {
        ModuleCommand::Check { directory } => {
            check::check(&directory)?;
            token.ensure_active()?;
            output.summary(format!("Checked module {}", display_path(&directory)))?;
        }
        ModuleCommand::Pack {
            source,
            output: destination,
        } => {
            let cancelled = BundleCancellation::from_flag(token.signal_flag());
            pack::pack(
                &source,
                &destination,
                &cancelled,
                |name, action, source, temp| match action {
                    Action::Nu { path, args } => {
                        run_nu(name, source, temp, path, args, &token, output)
                    }
                    Action::Argv(argv) => {
                        let diagnostics =
                            tola_build::hooks::run_before_pack(name, argv, source, temp, &token)?;
                        output.diagnostics(&diagnostics)?;
                        Ok(())
                    }
                },
            )?;
            output.summary(format!("Packed {}", display_path(&destination)))?;
        }
    }
    Ok(())
}

fn run_nu(
    name: &str,
    source: &Path,
    temp: &Path,
    path: &Path,
    args: &[String],
    cancelled: &tola_build::cancellation::BuildCancellation,
    output: &CommandOutput,
) -> Result<()> {
    let call = tola_build::hooks::before_pack_context(
        crate::cli::external::current_executable()?,
        name,
        source,
        temp,
    );
    let invocation = tola_nu::InvocationContext {
        directory: source.to_owned(),
        environment: super::nu::call_environment(tola_input::InputScope::Online, &call, temp)?,
    };
    let (submit, diagnostics) = super::nu::diagnostic::collector(
        format!("before-pack hook `{name}`"),
        ReportSources::new(),
    );
    let runtime = super::nu::prepare_runtime(vec![], vec![], invocation, vec![])?
        .with_cancellation(cancelled.signal_flag())
        .with_host(tola_nu::Host {
            submit_diagnostics: Some(submit),
            ..super::nu::call_host(Some(&call))?
        })?;
    let status = runtime.run_embedded(&source.join(path), args)?;
    cancelled.ensure_active()?;
    let diagnostics = diagnostics
        .lock()
        .expect("diagnostic collector")
        .take()
        .unwrap_or_default();
    if status != 0 {
        return Err(DiagnosticError::new(
            format!("before-pack hook `{name}` exited with status {status}"),
            diagnostics,
        )
        .into());
    }
    tola_diagnostic::ensure_reports_succeed(&diagnostics)?;
    output.diagnostics(&diagnostics)?;
    Ok(())
}
