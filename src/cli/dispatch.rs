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
        } => commands::build::run(
            config,
            packages,
            scope,
            build,
            crate::cli::config::build_resources(scope),
            output,
            cancellation,
        ),
        Commands::Dev {
            config,
            packages,
            build,
            development,
        } => commands::dev::run(
            config,
            packages,
            scope,
            build,
            development,
            crate::cli::config::build_resources(scope),
            output,
            cancellation,
        ),
        Commands::Preview {
            config,
            packages,
            build,
            server,
        } => commands::preview::run(
            config,
            packages,
            scope,
            build,
            server,
            crate::cli::config::build_resources(scope),
            output,
            cancellation,
        ),
        Commands::Check {
            config,
            packages,
            build,
        } => commands::check::run(
            config,
            packages,
            scope,
            build,
            crate::cli::config::build_resources(scope),
            output,
            cancellation,
        ),
        Commands::Inspect {
            config,
            packages,
            command,
        } => commands::inspect::run(
            config,
            packages,
            scope,
            command,
            crate::cli::config::build_resources(scope),
            output,
            cancellation,
        ),
        Commands::Config {
            config,
            packages,
            build,
        } => commands::config::run(config, packages, scope, build, output, cancellation),
        Commands::Vendor {
            config,
            packages,
            vendor,
        } => commands::vendor::run(
            config,
            packages,
            scope,
            vendor,
            crate::cli::config::build_resources(scope),
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
        Commands::Lsp { config, packages } => {
            commands::lsp::run(config, packages, scope, cancellation, output)
        }
        Commands::Skill(args) => commands::skill::run(args.output.as_deref(), output, cancellation),
        Commands::Help(args) => commands::help::run(
            &args,
            crate::i18n::HelpLanguage::resolve(cli.lang),
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
    if let Commands::Help(args) = command
        && let Some(directory) = &args.export
    {
        destination::check_directory(Some(log_path), directory)?;
    }
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
