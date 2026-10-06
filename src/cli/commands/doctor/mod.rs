use crate::cancellation::Cancellation;
mod checks;

use anyhow::Result;
use serde::Serialize;
use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, TypstPackageArgs};
use tola_build::InputScope;

pub(in crate::cli) fn run(
    config: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    json: bool,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let report = Report::collect(&config, &packages, scope, output, cancellation)?;
    cancellation.token().ensure_active()?;
    if json {
        output.write_stdout_line(serde_json::to_vec_pretty(&report)?)?;
    } else {
        let environment = &report.environment;
        output.status(format!(
            "Tola {} · Typst {} · {}/{}",
            environment.tola_version, environment.typst_version, environment.os, environment.arch,
        ))?;
    }
    if report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(DiagnosticError::new("site checks failed", report.diagnostics).into());
    }
    output.diagnostics(&report.diagnostics)?;
    if !json {
        output.summary("Doctor completed")?;
    }
    Ok(())
}

#[derive(Serialize)]
struct Report {
    environment: Environment,
    diagnostics: Vec<Diagnostic>,
}

impl Report {
    fn collect(
        config: &ConfigFileArgs,
        packages: &TypstPackageArgs,
        scope: InputScope,
        output: &CommandOutput,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        let diagnostics = match checks::diagnose(config, packages, scope, output, cancellation) {
            Ok(diagnostics) => diagnostics,
            Err(error) if error.is::<tola_build::cancellation::BuildCancelled>() => {
                return Err(error);
            }
            Err(error) => {
                crate::cli::output::attached_or_fallback(&error, crate::codes::doctor::CHECKS)
            }
        };
        Ok(Self {
            environment: Environment {
                tola_version: env!("CARGO_PKG_VERSION"),
                typst_version: typst::utils::version().raw(),
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            },
            diagnostics,
        })
    }
}

#[derive(Serialize)]
struct Environment {
    tola_version: &'static str,
    typst_version: &'static str,
    os: &'static str,
    arch: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_limit_hides_warnings() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("site.typ"), "").unwrap();
        let config = root.join("tola.toml");
        std::fs::write(&config, "[diagnostics]\nmax_warnings = 0\n").unwrap();
        let (sink, captured) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        run(
            ConfigFileArgs { path: Some(config) },
            TypstPackageArgs::default(),
            InputScope::default(),
            false,
            &output,
            &Cancellation::default(),
        )
        .unwrap();
        let shown = String::from_utf8(captured.bytes()).unwrap();
        assert!(shown.contains("1 warning not shown"));
        assert!(!shown.contains(crate::codes::doctor::EDITOR_PACKAGES_MISSING.as_str()));
    }

    #[test]
    fn json_report_lists_failures() {
        let directory = tempfile::tempdir().unwrap();
        let report = Report::collect(
            &ConfigFileArgs {
                path: Some(directory.path().join("missing.toml")),
            },
            &TypstPackageArgs::default(),
            InputScope::default(),
            &CommandOutput::new(
                crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
                None,
            ),
            &Cancellation::default(),
        )
        .unwrap();
        let encoded = serde_json::to_value(&report).unwrap();

        assert_eq!(
            encoded["environment"]["tola_version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(
            encoded["environment"]["typst_version"],
            typst::utils::version().raw()
        );
        assert!(
            encoded["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| {
                    diagnostic["severity"] == "error"
                        && diagnostic["code"] == serde_json::json!(tola_build::codes::config::IO)
                })
        );
    }
}
