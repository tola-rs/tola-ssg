//! The clap-facing help and signatures of module commands.

use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Args;
use tola_input::InputScope;
use tola_module::{Action, CommandAction, ModuleCommands};

use crate::cli::args::SkillArgs;

pub(super) fn is_help(arguments: &[OsString]) -> bool {
    matches!(
        arguments.first().and_then(|argument| argument.to_str()),
        Some("--help" | "-h")
    )
}

pub(super) fn namespace_command(module: &ModuleCommands) -> clap::Command {
    let mut command = super::namespace_options(&module.name).subcommand_required(true);
    if module.initializer.is_some() {
        command =
            command.subcommand(clap::Command::new("init").about("Create a site using this module"));
    }
    for (name, declared) in &module.commands {
        let description = declared.help.lines().next().unwrap_or("");
        command =
            command.subcommand(clap::Command::new(name.clone()).about(description.to_owned()));
    }
    command
}

pub(super) fn namespace_help(module: &ModuleCommands, use_color: bool, long: bool) -> String {
    render_help(namespace_command(module), use_color, long)
}

pub(super) fn command_signature(
    module: &ModuleCommands,
    name: &str,
    project: &Path,
    config: &Path,
    scope: InputScope,
) -> Option<Result<tola_nu::Signature>> {
    let declared = module.commands.get(name)?;
    let CommandAction::Execute(Action::Nu { path, args }) = declared
        .action()
        .expect("module command actions are validated")
    else {
        return None;
    };
    let path = match tola_module::module_set::validate_module_path(
        project,
        &module.name,
        &module.root,
        &path,
        false,
    ) {
        Ok(path) => path,
        Err(error) => return Some(Err(error.into())),
    };
    let signature = super::super::nu::command_signatures(&[(path, args)], project, config, scope)
        .pop()
        .expect("one signature result per script");
    let mut signature = match signature {
        Ok(signature) => signature?,
        Err(error) => return Some(Err(error)),
    };
    if !declared.help.trim().is_empty() {
        let (short, long) = declared
            .help
            .split_once('\n')
            .unwrap_or((&declared.help, ""));
        signature.description = short.to_owned();
        signature.extra_description = long.trim().to_owned();
    }
    Some(Ok(signature))
}

pub(super) fn render_help(mut command: clap::Command, use_color: bool, long: bool) -> String {
    command = command.color(if use_color {
        clap::ColorChoice::Always
    } else {
        clap::ColorChoice::Never
    });
    let rendered = if long {
        command.render_long_help()
    } else {
        command.render_help()
    };
    if use_color {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    }
}

fn named_command(module: &ModuleCommands, name: &str) -> clap::Command {
    clap::Command::new(name.to_owned()).bin_name(format!("tola {} {name}", module.name))
}

fn default_str(val: Option<tola_nu::Value>) -> Option<String> {
    val.and_then(|v| v.coerce_string().ok())
}

pub(super) fn command_help(
    module: &ModuleCommands,
    name: &str,
    declared: &tola_module::Command,
    use_color: bool,
    long: bool,
) -> String {
    let cmd = named_command(module, name)
        .about(declared.help.lines().next().unwrap_or("").to_owned())
        .long_about(declared.help.clone());
    let action = declared
        .action()
        .expect("module command actions are validated");
    let cmd = match &action {
        CommandAction::Query { .. } => super::super::query::QueryOptions::augment_args(cmd),
        CommandAction::Execute(_) => cmd.arg(
            clap::Arg::new("ARGS")
                .num_args(0..)
                .allow_hyphen_values(true)
                .help("Arguments passed to the command"),
        ),
        CommandAction::Skill { .. } => SkillArgs::augment_args(cmd),
    };
    render_help(cmd, use_color, long)
}

pub(super) fn skill_command(module: &ModuleCommands, name: &str) -> clap::Command {
    SkillArgs::augment_args(named_command(module, name))
}

pub(super) fn signature_command(
    module: &ModuleCommands,
    name: &str,
    sig: tola_nu::Signature,
) -> clap::Command {
    let long_about = if sig.extra_description.is_empty() {
        sig.description.clone()
    } else {
        format!("{}\n\n{}", sig.description, sig.extra_description)
    };
    let mut cmd = named_command(module, name)
        .about(sig.description)
        .long_about(long_about);
    let pos = sig
        .required_positional
        .into_iter()
        .map(|arg| (arg, true, false))
        .chain(
            sig.optional_positional
                .into_iter()
                .map(|a| (a, false, false)),
        )
        .chain(sig.rest_positional.into_iter().map(|a| (a, false, true)));
    for (p, req, rest) in pos {
        let mut arg = clap::Arg::new(p.name.clone())
            .value_name(p.name.to_uppercase())
            .help(p.desc)
            .required(req);
        if rest {
            arg = arg.num_args(0..);
        }
        if let Some(def) = default_str(p.default_value) {
            arg = arg.default_value(def);
        }
        cmd = cmd.arg(arg);
    }
    for flag in sig.named {
        if flag.long == "help" || flag.short == Some('h') {
            continue;
        }
        let id = if flag.long.is_empty() {
            flag.short.map(|s| s.to_string()).unwrap_or_default()
        } else {
            flag.long.clone()
        };
        let mut arg = clap::Arg::new(id.clone())
            .help(flag.desc)
            .required(flag.required);
        if !flag.long.is_empty() {
            arg = arg.long(flag.long.clone());
        }
        if let Some(short) = flag.short {
            arg = arg.short(short);
        }
        arg = if flag.arg.is_some() {
            let mut arg = arg.value_name(id.to_uppercase());
            if let Some(def) = default_str(flag.default_value) {
                arg = arg.default_value(def);
            }
            arg
        } else {
            arg.action(clap::ArgAction::SetTrue)
        };
        cmd = cmd.arg(arg);
    }
    cmd
}
