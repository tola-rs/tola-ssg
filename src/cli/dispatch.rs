//! Dispatch from parsed arguments to command-specific adapters.

use crate::cancellation::Cancellation;
use crate::cli::log::LogFile;
use crate::cli::log::destination;
use anyhow::Result;

use super::{Cli, Commands, commands};
use crate::cli::output::CommandOutput;

pub(crate) fn dispatch(
    cli: Cli,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    validate_log_outputs(&cli.command, output)?;
    let scope = cli.input_scope();
    match cli.command {
        Commands::Init(args) => commands::init::run(&args, scope, output, cancellation),
        Commands::Build {
            config,
            packages,
            build,
            fonts,
        } => commands::build::run(commands::SiteRequest {
            config_file: config,
            packages,
            scope,
            build,
            resources: crate::cli::config::build_resources_with_fonts(scope, &fonts)?,
            output,
            cancellation,
        }),
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
