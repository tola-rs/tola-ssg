//! Dispatch from parsed arguments to command-specific adapters.

use crate::cancellation::Cancellation;
use crate::cli::log::destination;
use anyhow::Result;

use super::parse::{Command, ParsedCommand};
use super::{Commands, commands};
use crate::cli::BuildArgs;
use crate::cli::output::CommandOutput;
use tola_input::InputScope;

pub(super) fn dispatch(
    command: Command,
    output: &CommandOutput,
    cancellation: &Cancellation,
    failure: Option<FailureReport>,
) -> Result<()> {
    if let Err(error) = cancellation.token().ensure_active() {
        return report_failure(failure, output, error.into());
    }
    let mode = command.input_mode();
    let scope = mode.scope();
    let command = match command {
        Command::Builtin(command) => command,
        Command::Extension(args) => {
            return commands::extensions::run(args, mode, output, cancellation);
        }
    };
    match command {
        Commands::Nu(_) => unreachable!("script runs before cancellation is installed"),
        Commands::Extension(_) => unreachable!("extension arguments are parsed before dispatch"),
        Commands::Update {
            input: _,
            site,
            update,
        } => commands::update::run(
            site.config,
            site.packages,
            mode,
            update,
            output,
            cancellation,
        ),
        Commands::Module { command } => commands::module::run(command, output, cancellation),
        Commands::Init(args) => commands::init::run(&args, mode, output, cancellation),
        Commands::Edit {
            input: _,
            site,
            command,
        } => commands::edit::run(site.config, command, output, cancellation),
        Commands::Build {
            input: _,
            site,
            build,
            json,
            pretty,
        } => {
            let request = match commands::SiteRequest::new(site, build, mode, output, cancellation)
            {
                Ok(request) => request,
                Err(error) => return report_failure(failure, output, error),
            };
            commands::build::run(request, json, pretty)
        }
        Commands::Dev {
            input: _,
            site,
            build,
            development,
        } => commands::dev::run(
            commands::SiteRequest::new(site, build, mode, output, cancellation)?,
            development,
        ),
        Commands::Preview {
            input: _,
            site,
            build,
            server,
        } => commands::preview::run(
            commands::SiteRequest::new(site, build, mode, output, cancellation)?,
            server,
        ),
        Commands::Check {
            input: _,
            site,
            build,
            no_build,
            no_hooks,
            json,
            pretty,
        } => {
            let request = match commands::SiteRequest::new(site, build, mode, output, cancellation)
            {
                Ok(request) => request,
                Err(error) => return report_failure(failure, output, error),
            };
            if no_build {
                commands::check::sources(&request, json, pretty)
            } else {
                commands::check::run(request, json, no_hooks, pretty)
            }
        }
        Commands::Fmt {
            input: _,
            site,
            check,
        } => commands::fmt::run(
            site.config,
            site.packages,
            scope,
            check,
            output,
            cancellation,
        ),
        Commands::Inspect {
            input: _,
            site,
            command,
            fonts,
        } => commands::inspect::run(
            commands::SiteRequest::new(
                site,
                BuildArgs::from_fonts(fonts),
                mode,
                output,
                cancellation,
            )?,
            command,
        ),
        Commands::Query {
            input: _,
            site,
            fonts,
            query,
        } => {
            let request = commands::SiteRequest::new(
                site,
                BuildArgs::from_fonts(fonts),
                mode,
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
            input: _,
            site,
            vendor,
            fonts,
        } => commands::vendor::run(
            site.config,
            site.packages,
            scope,
            vendor,
            crate::cli::config::build_resources_with_fonts(mode, &fonts)?,
            output,
            cancellation,
        ),
        Commands::Doctor(args) => commands::doctor::run(
            args.site.config,
            args.site.packages,
            scope,
            args.json,
            args.pretty,
            output,
            cancellation,
        ),
        Commands::Editor { command } => commands::editor::run(command, scope, output, cancellation),
        Commands::Lsp {
            input: _,
            site,
            fonts,
        } => commands::lsp::run(
            site.config,
            site.packages,
            fonts,
            mode,
            cancellation,
            output,
        ),
        Commands::Skill(args) => commands::skill::run_builtin(&args, output, cancellation),
        Commands::Version(args) => commands::version::run(&args, output),
        Commands::Help(args) => commands::help::run(
            &args,
            args.lang.unwrap_or_default(),
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

/// The JSON report a build or check can produce even if setup fails.
#[derive(Clone, Copy)]
pub(super) struct FailureReport {
    command: &'static str,
    scope: InputScope,
    stage: &'static str,
    hooks: bool,
    pretty: bool,
}

impl FailureReport {
    pub(super) fn of(parsed: &ParsedCommand) -> Option<Self> {
        let Command::Builtin(command) = &parsed.command else {
            return None;
        };
        let scope = parsed.command.input_mode().scope();
        match command {
            Commands::Build {
                json: true, pretty, ..
            } => Some(Self {
                command: "build",
                scope,
                stage: "candidate",
                hooks: true,
                pretty: *pretty,
            }),
            Commands::Check {
                json: true,
                pretty,
                no_build,
                no_hooks,
                ..
            } => Some(Self {
                command: "check",
                scope,
                stage: if *no_build { "source" } else { "candidate" },
                hooks: !*no_build && !*no_hooks,
                pretty: *pretty,
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
        .finish(Err(error), output, true, failure.pretty)
}
