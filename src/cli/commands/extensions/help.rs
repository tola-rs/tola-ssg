//! The clap-facing help and signatures of module commands.

use std::collections::BTreeMap;
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

pub(super) fn namespace_command(
    module: &ModuleCommands,
    signatures: &BTreeMap<String, Result<tola_nu::Signature>>,
) -> clap::Command {
    let mut command = super::namespace_options(&module.name).subcommand_required(true);
    if module.initializer.is_some() {
        command =
            command.subcommand(clap::Command::new("init").about("Create a site using this module"));
    }
    for (name, declared) in &module.commands {
        let description = signatures
            .get(name)
            .and_then(|signature| signature.as_ref().ok())
            .map(|signature| signature.description.as_str())
            .unwrap_or(&declared.help);
        command = command.subcommand(
            clap::Command::new(name.clone())
                .about(description.lines().next().unwrap_or("").to_owned()),
        );
    }
    command
}

pub(super) fn namespace_help(
    module: &ModuleCommands,
    signatures: &BTreeMap<String, Result<tola_nu::Signature>>,
    use_color: bool,
    long: bool,
) -> String {
    render_help(namespace_command(module, signatures), use_color, long)
}

pub(super) fn command_signatures<'a>(
    module: &'a ModuleCommands,
    names: impl Iterator<Item = &'a str>,
    project: &Path,
    config: &Path,
    scope: InputScope,
) -> Result<BTreeMap<String, Result<tola_nu::Signature>>> {
    let commands = names.map(|name| (module, name));
    Ok(module_command_signatures(commands, project, config, scope)?
        .into_iter()
        .map(|((_, name), signature)| (name, signature))
        .collect())
}

/// One module command's argv spelling and its resolved signature.
pub(in crate::cli) type ModuleCommandSignature = ((String, String), Result<tola_nu::Signature>);

pub(in crate::cli) fn module_command_signatures<'a>(
    commands: impl Iterator<Item = (&'a ModuleCommands, &'a str)>,
    project: &Path,
    config: &Path,
    scope: InputScope,
) -> Result<Vec<ModuleCommandSignature>> {
    let selected = commands
        .filter_map(|(module, name)| {
            let declared = module.commands.get(name)?;
            let CommandAction::Execute(Action::Nu { path, args }) = declared
                .action()
                .expect("module command actions are validated")
            else {
                return None;
            };
            let path = tola_module::module_set::validate_module_path(
                project,
                &module.name,
                &module.root,
                &path,
                false,
            );
            Some((module, name, declared, (path, args)))
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let invocations = selected
        .iter()
        .filter_map(|(_, _, _, (path, args))| {
            path.as_ref().ok().map(|path| (path.clone(), args.clone()))
        })
        .collect::<Vec<_>>();
    let mut signatures =
        super::super::nu::command_signatures(&invocations, project, config, scope).into_iter();
    Ok(selected
        .into_iter()
        .filter_map(|(module, name, declared, (path, _))| {
            let signature = match path {
                Ok(_) => signatures
                    .next()
                    .expect("one signature per valid script path"),
                Err(error) => Err(error.into()),
            };
            let key = (module.name.clone(), name.to_owned());
            match signature {
                Ok(None) => None,
                Err(error) => Some((key, Err(error))),
                Ok(Some(mut signature)) => {
                    if !declared.help.trim().is_empty() {
                        signature.description =
                            declared.help.lines().next().unwrap_or("").to_owned();
                        signature.extra_description = declared
                            .help
                            .lines()
                            .skip(1)
                            .collect::<Vec<_>>()
                            .join("\n")
                            .trim()
                            .to_owned();
                    }
                    Some((key, Ok(signature)))
                }
            }
        })
        .collect())
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

pub(super) fn command_help(
    module: &ModuleCommands,
    name: &str,
    declared: &tola_module::Command,
    use_color: bool,
    long: bool,
) -> String {
    let command = clap::Command::new(name.to_owned())
        .bin_name(format!("tola {} {name}", module.name))
        .about(declared.help.lines().next().unwrap_or("").to_owned())
        .long_about(declared.help.clone());
    let action = declared
        .action()
        .expect("module command actions are validated");
    let command = match &action {
        CommandAction::Query { .. } => super::super::query::QueryOptions::augment_args(command),
        CommandAction::Execute(_) => command.arg(
            clap::Arg::new("ARGS")
                .num_args(0..)
                .allow_hyphen_values(true)
                .help("Arguments passed to the command"),
        ),
        CommandAction::Skill { .. } => SkillArgs::augment_args(command),
    };
    render_help(command, use_color, long)
}

pub(super) fn skill_command(module: &ModuleCommands, name: &str) -> clap::Command {
    let command =
        clap::Command::new(name.to_owned()).bin_name(format!("tola {} {name}", module.name));
    SkillArgs::augment_args(command)
}

pub(super) fn signature_command(
    module: &ModuleCommands,
    name: &str,
    signature: tola_nu::Signature,
) -> clap::Command {
    let long_about = if signature.extra_description.is_empty() {
        signature.description.clone()
    } else {
        format!(
            "{}\n\n{}",
            signature.description, signature.extra_description
        )
    };
    let mut command = clap::Command::new(name.to_owned())
        .bin_name(format!("tola {} {name}", module.name))
        .about(signature.description)
        .long_about(long_about);
    for (positional, required, rest) in signature
        .required_positional
        .into_iter()
        .map(|arg| (arg, true, false))
        .chain(
            signature
                .optional_positional
                .into_iter()
                .map(|arg| (arg, false, false)),
        )
        .chain(
            signature
                .rest_positional
                .into_iter()
                .map(|arg| (arg, false, true)),
        )
    {
        let mut arg = clap::Arg::new(positional.name.clone())
            .value_name(positional.name.to_uppercase())
            .help(positional.desc)
            .required(required);
        if rest {
            arg = arg.num_args(0..);
        }
        if let Some(default) = positional
            .default_value
            .and_then(|value| value.coerce_string().ok())
        {
            arg = arg.default_value(default);
        }
        command = command.arg(arg);
    }
    for flag in signature.named {
        if flag.long == "help" || flag.short == Some('h') {
            continue;
        }
        let id = if flag.long.is_empty() {
            flag.short
                .map(|short| short.to_string())
                .unwrap_or_default()
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
            if let Some(default) = flag
                .default_value
                .and_then(|value| value.coerce_string().ok())
            {
                arg = arg.default_value(default);
            }
            arg
        } else {
            arg.action(clap::ArgAction::SetTrue)
        };
        command = command.arg(arg);
    }
    command
}
