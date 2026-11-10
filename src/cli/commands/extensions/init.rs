//! The init subcommand a module declares.

use std::ffi::OsString;

use anyhow::Result;
use clap::{Args, FromArgMatches};
use tola_input::InputScope;
use tola_module::ModuleCommands;

use super::help::{is_help, render_help};
use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;

pub(super) fn run_initializer(
    module: &ModuleCommands,
    tail: &[OsString],
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let command = crate::cli::args::InitArgs::augment_args(
        clap::Command::new("init").bin_name(format!("tola {} init", module.name)),
    )
    .mut_arg("from", |arg| arg.hide(true))
    .mut_arg("alias", |arg| arg.hide(true));
    if is_help(tail) {
        return output.write_stdout(render_help(
            command.clone(),
            output.terminal().stdout_uses_color(),
            tail[0] == "--help",
        ));
    }
    let arguments = [
        OsString::from("init"),
        OsString::from("--from"),
        module.root.as_os_str().to_owned(),
        OsString::from("--alias"),
        OsString::from(&module.name),
    ];
    let matches = command.try_get_matches_from(arguments.iter().chain(tail))?;
    let args = crate::cli::args::InitArgs::from_arg_matches(&matches)?;
    super::super::init::run(&args, scope, output, cancellation)
}
