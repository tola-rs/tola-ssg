//! Convert clap matches into the command and process options.

use std::process::ExitCode;

use clap::{
    CommandFactory, FromArgMatches,
    builder::StyledStr,
    error::{ContextKind, ContextValue, ErrorKind},
};
use tola_input::InputMode;

use super::args::{Cli, Commands, ConfigFileArgs, ProcessArgs};
use super::commands::extensions;

pub(super) struct ParsedCommand {
    pub name: String,
    pub process: ProcessArgs,
    pub command: Command,
}

pub(super) enum Command {
    Builtin(Commands),
    Extension(extensions::ExtensionArgs),
}

impl Command {
    pub fn input_mode(&self) -> InputMode {
        match self {
            Self::Builtin(command) => command.input_mode(),
            Self::Extension(args) => args.input.mode(),
        }
    }

    pub fn config_file(&self) -> Option<&ConfigFileArgs> {
        match self {
            Self::Builtin(command) => command.config_file(),
            Self::Extension(args) => Some(&args.config),
        }
    }
}

impl ParsedCommand {
    pub fn parse(arguments: &[std::ffi::OsString]) -> Result<Self, ExitCode> {
        let mut command = Cli::command();
        if arguments.len() == 1 {
            return Err(super::usage::root(command.render_help()));
        }
        let matches = match command.try_get_matches_from_mut(arguments) {
            Ok(matches) => matches,
            Err(error) if error.kind() == ErrorKind::DisplayHelp => {
                return Err(super::usage::render(arguments, error));
            }
            Err(mut error) => {
                add_placement_hint(&mut error, arguments);
                error.exit();
            }
        };
        let name = matches.subcommand_name().expect("clap requires a command");
        let Cli { command } = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
        let (command, process) = match command {
            Commands::Extension(arguments) => {
                let (args, process) = extensions::ExtensionArgs::parse(name, &arguments[1..])
                    .unwrap_or_else(|error| error.exit());
                (Command::Extension(args), process)
            }
            command => {
                let process = command.process();
                (Command::Builtin(command), process)
            }
        };
        Ok(Self {
            name: name.to_owned(),
            process,
            command,
        })
    }
}

fn add_placement_hint(error: &mut clap::Error, arguments: &[std::ffi::OsString]) {
    if error.kind() != ErrorKind::UnknownArgument
        || arguments.get(1).and_then(|arg| arg.to_str()) != Some("inspect")
        || !matches!(
            error.get(ContextKind::InvalidArg),
            Some(ContextValue::String(arg)) if matches!(arg.as_str(), "-C" | "--config")
        )
    {
        return;
    }
    let mut suggestion = StyledStr::new();
    suggestion.push_str("Place --config <FILE> after 'inspect', before the inspection command");
    error.insert(
        ContextKind::Suggested,
        ContextValue::StyledStrs(vec![suggestion]),
    );
}
