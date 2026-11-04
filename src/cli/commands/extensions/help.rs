//! The clap-facing help and signatures of module commands.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;
use clap::Args;
use tola_build::InputScope;
use tola_build::modules::{CommandAction, ModuleCommands};

use super::argv::expand;

pub(super) fn is_help(arguments: &[OsString]) -> bool {
    arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h")
}

pub(super) fn namespace_command(
    module: &ModuleCommands,
    signatures: &BTreeMap<String, tola_nu::Signature>,
) -> clap::Command {
    let mut command = clap::Command::new(module.name.clone())
        .bin_name(format!("tola {}", module.name))
        .about(module.summary.clone())
        .disable_help_subcommand(true)
        .subcommand_required(true)
        .arg(
            clap::Arg::new("config")
                .short('C')
                .long("config")
                .value_name("FILE")
                .help("Use this site configuration"),
        );
    if module.initializer.is_some() {
        command =
            command.subcommand(clap::Command::new("init").about("Create a site using this module"));
    }
    for (name, declared) in &module.commands {
        let description = signatures
            .get(name)
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
    signatures: &BTreeMap<String, tola_nu::Signature>,
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
) -> Result<BTreeMap<String, tola_nu::Signature>> {
    let commands = names.map(|name| (module, name));
    module_command_signatures(commands, project, config, scope)?
        .into_iter()
        .map(|((_, name), signature)| signature.map(|signature| (name, signature)))
        .collect()
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
            let CommandAction::Argv { argv } = &declared.action else {
                return None;
            };
            (argv.first().is_some_and(|arg| arg == "{tola}")
                && argv.get(1).is_some_and(|arg| arg == "nu"))
            .then_some((module, name, declared, argv))
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let executable = crate::cli::external::current_executable()?;
    let invocations = selected
        .iter()
        .map(|(module, _, _, argv)| {
            (
                argv.iter()
                    .map(|arg| expand(arg, project, &module.root, &executable))
                    .collect(),
                module.root.clone(),
            )
        })
        .collect::<Vec<_>>();
    let signatures = super::super::nu::command_signatures(&invocations, project, config, scope);
    Ok(selected
        .into_iter()
        .zip(signatures)
        .filter_map(|((module, name, declared, _), signature)| {
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
    declared: &tola_build::modules::Command,
    use_color: bool,
    long: bool,
) -> String {
    let command = clap::Command::new(name.to_owned())
        .bin_name(format!("tola {} {name}", module.name))
        .about(declared.help.lines().next().unwrap_or("").to_owned())
        .long_about(declared.help.clone());
    let command = match &declared.action {
        CommandAction::Query { .. } => super::super::query::QueryOptions::augment_args(command),
        CommandAction::Argv { .. } => command.arg(
            clap::Arg::new("ARGS")
                .num_args(0..)
                .allow_hyphen_values(true)
                .help("Arguments passed to the command"),
        ),
    };
    render_help(command, use_color, long)
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

