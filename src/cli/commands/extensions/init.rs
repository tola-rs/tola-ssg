//! The init subcommand a module declares.

use std::ffi::OsString;

use anyhow::Result;
use clap::{Args, FromArgMatches};
use tola_input::InputMode;
use tola_module::ModuleCommands;

use super::help::{is_help, render_help};
use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;

pub(super) fn run_initializer(
    module: &ModuleCommands,
    tail: &[OsString],
    config: &tola_build::config::ResolvedSiteConfig,
    mode: InputMode,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let mut command = crate::cli::args::InitArgs::augment_args(
        clap::Command::new("init").bin_name(format!("tola {} init", module.name)),
    )
    .mut_arg("from", |arg| arg.hide(true))
    .mut_arg("alias", |arg| arg.hide(true));
    for name in ["branch", "tag", "rev", "subdir"] {
        command = command.mut_arg(name, |arg| arg.hide(true).conflicts_with("from"));
    }
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
    let scope = match (mode.scope(), args.input.scope()) {
        (tola_input::InputScope::Pure, _) | (_, tola_input::InputScope::Pure) => {
            tola_input::InputScope::Pure
        }
        (tola_input::InputScope::Offline, _) | (_, tola_input::InputScope::Offline) => {
            tola_input::InputScope::Offline
        }
        _ => tola_input::InputScope::Online,
    };
    let mode = InputMode::new(scope, mode.locked() || args.input.locked);
    let locked = tola_input::InputLock::read(config.get_root())?
        .and_then(|lock| lock.modules.get(&module.name).cloned());
    let source = super::super::init::Source::selected(
        &config.modules()[&module.name].source,
        config.get_root(),
        &module.root,
        locked,
    );
    super::super::init::run_with_source(&args, Some(source), mode, output, cancellation)
}
