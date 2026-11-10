//! Convert clap matches into the command and process options.

use std::process::ExitCode;

use clap::{ColorChoice, CommandFactory, FromArgMatches, error::ErrorKind};
use tola_input::InputMode;

use super::args::{Cli, Commands, ConfigFileArgs, ProcessArgs};
use super::commands::extensions;
use crate::terminal::color::{Stream, enabled};

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
            Self::Builtin(command) => command.input().copied().unwrap_or_default().mode(),
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
    pub fn parse() -> Result<Self, ExitCode> {
        let arguments = std::env::args_os().collect::<Vec<_>>();
        let color = super::argv::color_hint(&arguments);
        let mut command = Cli::command().color(stream_color(color, Stream::Stdout));
        if arguments.len() == 1 {
            return Err(super::usage::root(command.render_help(), color));
        }
        let matches = match command.try_get_matches_from_mut(&arguments) {
            Ok(matches) => matches,
            Err(error) if error.kind() == ErrorKind::DisplayHelp => {
                return Err(super::usage::render(&arguments, error));
            }
            Err(error) if error.use_stderr() => {
                error
                    .with_cmd(&command.color(stream_color(color, Stream::Stderr)))
                    .exit();
            }
            Err(error) => error.exit(),
        };
        let name = matches.subcommand_name().expect("clap requires a command");
        let Cli {
            mut global,
            command,
        } = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
        let command = match command {
            Commands::Extension(arguments) => Command::Extension(
                extensions::ExtensionArgs::parse(name, &arguments[1..], &mut global)
                    .unwrap_or_else(|error| error.exit()),
            ),
            command => Command::Builtin(command),
        };
        Ok(Self {
            name: name.to_owned(),
            process: global,
            command,
        })
    }
}

pub(super) fn stream_color(color: ColorChoice, stream: Stream) -> ColorChoice {
    if enabled(color, stream) {
        ColorChoice::Always
    } else {
        ColorChoice::Never
    }
}
