//! Dispatch from parsed arguments to command-specific adapters.

use crate::cancellation::Cancellation;
use crate::cli::log::LogFile;
use crate::cli::log::destination;
use anyhow::Result;

use super::{Cli, Commands, commands};
use crate::cli::output::CommandOutput;
use crate::cli::{BuildOverrideArgs, CompilerFontArgs, TypstPackageArgs};
use tola_build::InputScope;

pub(crate) fn dispatch(
    cli: Cli,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let failure = FailureReport::of(&cli);
    if let Err(error) = cancellation.token().ensure_active() {
        return report_failure(failure, output, error.into());
    }
    if let Err(error) = validate_log_outputs(&cli.command, output) {
        return report_failure(failure, output, error);
    }
    let scope = cli.input_scope();
    match cli.command {
        Commands::Nu(_) => unreachable!("script runs before cancellation is installed"),
        Commands::Extension(arguments) => {
            commands::extensions::run(arguments, scope, output, cancellation)
        }
        Commands::Module { config, command } => {
            let request = commands::SiteRequest::new(
                config,
                TypstPackageArgs::default(),
                scope,
                BuildOverrideArgs::default(),
                CompilerFontArgs::default(),
                output,
                cancellation,
            )?;
            let loaded = request.load_config(&crate::config::ConfigOverrides::default())?;
            let super::args::ModuleCommand::Fetch { alias } = command;
            output.status("Fetching remote modules")?;
            tola_build::modules::fetch_modules(
                loaded.config().get_root(),
                loaded.config().modules(),
                alias.as_deref(),
                scope,
                &cancellation.token(),
            )?;
            output.status("Remote modules are ready")?;
            Ok(())
        }
        Commands::Init(args) => commands::init::run(&args, scope, output, cancellation),
        Commands::Edit { config, command } => {
            commands::edit::run(config, command, output, cancellation)
        }
        Commands::Build {
            config,
            packages,
            build,
            fonts,
            json,
        } => {
            let request = match commands::SiteRequest::new(
                config,
                packages,
                scope,
                build,
                fonts,
                output,
                cancellation,
            ) {
                Ok(request) => request,
                Err(error) => return report_failure(failure, output, error),
            };
            commands::build::run(request, json)
        }
        Commands::Dev {
            config,
            packages,
            build,
            fonts,
            development,
        } => commands::dev::run(
            commands::SiteRequest::new(
                config,
                packages,
                scope,
                build,
                fonts,
                output,
                cancellation,
            )?,
            development,
        ),
        Commands::Preview {
            config,
            packages,
            build,
            fonts,
            server,
        } => commands::preview::run(
            commands::SiteRequest::new(
                config,
                packages,
                scope,
                build,
                fonts,
                output,
                cancellation,
            )?,
            server,
        ),
        Commands::Check {
            config,
            packages,
            build,
            fonts,
            no_build,
            no_hooks,
            json,
        } => {
            let request = match commands::SiteRequest::new(
                config,
                packages,
                scope,
                build,
                fonts,
                output,
                cancellation,
            ) {
                Ok(request) => request,
                Err(error) => return report_failure(failure, output, error),
            };
            if no_build {
                commands::check::sources(&request, json)
            } else {
                commands::check::run(request, json, no_hooks)
            }
        }
        Commands::Fmt {
            config,
            packages,
            check,
        } => commands::fmt::run(config, packages, scope, check, output, cancellation),
        Commands::Inspect {
            config,
            packages,
            command,
            fonts,
        } => commands::inspect::run(
            commands::SiteRequest::new(
                config,
                packages,
                scope,
                BuildOverrideArgs::default(),
                fonts,
                output,
                cancellation,
            )?,
            command,
        ),
        Commands::Query {
            config,
            packages,
            fonts,
            query,
        } => {
            let request = commands::SiteRequest::new(
                config,
                packages,
                scope,
                BuildOverrideArgs::default(),
                fonts,
                output,
                cancellation,
            )?;
            let loaded = request.load_config(&crate::config::ConfigOverrides::default())?;
            commands::query::run(
                std::sync::Arc::new(loaded.into_config()),
                &query,
                request.resources,
                output,
                cancellation,
            )
        }
        Commands::Vendor {
            config,
            packages,
            vendor,
            fonts,
        } => commands::vendor::run(
            config,
            packages,
            scope,
            vendor,
            crate::cli::config::build_resources_with_fonts(scope, &fonts)?,
            output,
            cancellation,
        ),
        Commands::Doctor(args) => commands::doctor::run(
            args.config,
            args.packages,
            scope,
            args.json,
            output,
            cancellation,
        ),
        Commands::Editor { command } => commands::editor::run(command, scope, output, cancellation),
        Commands::Lsp {
            config,
            packages,
            fonts,
        } => commands::lsp::run(config, packages, fonts, scope, cancellation, output),
        Commands::Skill(args) => commands::skill::run(&args, output, cancellation),
        Commands::Help(args) => commands::help::run(
            &args,
            cli.lang.unwrap_or_default(),
            scope,
            output,
            cancellation,
        ),
        Commands::Diagnostic {
            command: super::args::DiagnosticCommand::Render,
        } => commands::diagnostic::render(output),
        Commands::Completions(args) => {
            destination::start(output, &cancellation.token())?;
            commands::completions::run(args.shell, output)
        }
        Commands::Manpage(args) => {
            destination::start(output, &cancellation.token())?;
            commands::manpage::run(args.output.as_deref(), output)
        }
    }
}

/// The machine-readable report a failed invocation produces, for commands that answer in JSON.
#[derive(Clone, Copy)]
pub(super) struct FailureReport {
    command: &'static str,
    scope: InputScope,
    stage: &'static str,
    hooks: bool,
}

impl FailureReport {
    pub(super) fn of(cli: &Cli) -> Option<Self> {
        match &cli.command {
            Commands::Build { json: true, .. } => Some(Self {
                command: "build",
                scope: cli.input_scope(),
                stage: "candidate",
                hooks: true,
            }),
            Commands::Check {
                json: true,
                no_build,
                no_hooks,
                ..
            } => Some(Self {
                command: "check",
                scope: cli.input_scope(),
                stage: if *no_build { "source" } else { "candidate" },
                hooks: !*no_build && !*no_hooks,
            }),
            _ => None,
        }
    }
}

pub(super) fn report_failure(
    failure: Option<FailureReport>,
    output: &CommandOutput,
    error: anyhow::Error,
) -> Result<()> {
    let Some(failure) = failure else {
        return Err(error);
    };
    commands::report::SiteReport::new(failure.command, failure.scope, failure.stage, failure.hooks)
        .finish(Err(error), output, true)
}

fn validate_log_outputs(command: &Commands, output: &CommandOutput) -> Result<()> {
    let Some(log_path) = output.log().map(LogFile::path) else {
        return Ok(());
    };
    if let Some(config) = command.config_file() {
        let cwd = std::env::current_dir()?;
        let path = config.path.clone().unwrap_or_else(|| {
            tola_build::config::loading::find_default_config(&cwd)
                .unwrap_or_else(|| cwd.join("tola.toml"))
        });
        destination::check_file(Some(log_path), &path)?;
    }
    if let Some(path) = command.written_output() {
        destination::check_file(Some(log_path), path)?;
    }
    Ok(())
}
