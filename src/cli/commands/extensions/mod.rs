//! Commands explicitly declared by the site's local modules.

use std::collections::BTreeMap;
use std::ffi::OsString;

use anyhow::{Context, Result, ensure};
use clap::{Args, CommandFactory, FromArgMatches, parser::ValueSource};
use tola_build::report::{REPORT_PATH_ENV, ReportChannel, ReportSources};
use tola_input::InputMode;
use tola_module::{Action, CommandAction};

use crate::cancellation::Cancellation;
use crate::cli::args::{InputArgs, ProcessArgs};
use crate::cli::output::CommandOutput;
use crate::cli::usage::{HelpLength, help_flag, observe_help};
use crate::cli::{Cli, ConfigFileArgs, TypstPackageArgs};

mod argv;
mod help;
mod init;

use argv::{complete_report, process_command};
pub(in crate::cli) use help::module_command_signatures;
use help::{
    command_help, command_signatures, is_help, namespace_command, namespace_help, render_help,
    signature_command,
};
use init::run_initializer;

pub(in crate::cli) struct ExtensionArgs {
    namespace: String,
    pub(in crate::cli) config: ConfigFileArgs,
    pub(in crate::cli) input: InputArgs,
    help: Option<HelpLength>,
    command_args: Vec<OsString>,
}

#[derive(Args)]
struct NamespaceOptions {
    #[command(flatten)]
    process: ProcessArgs,
    #[command(flatten)]
    input: InputArgs,
    #[command(flatten)]
    config: ConfigFileArgs,
}

fn namespace_options(namespace: &str) -> clap::Command {
    NamespaceOptions::augment_args(
        clap::Command::new(namespace.to_owned())
            .bin_name(format!("tola {namespace}"))
            .disable_help_subcommand(true),
    )
    .about(None)
    .long_about(None)
}

impl ExtensionArgs {
    pub(in crate::cli) fn parse(
        namespace: &str,
        arguments: &[OsString],
        global: &mut ProcessArgs,
    ) -> Result<Self, clap::Error> {
        let command = namespace_options(namespace)
            .color(crate::cli::parse::stream_color(
                global.color,
                crate::terminal::color::Stream::Stderr,
            ))
            .arg(
                clap::Arg::new("command_args")
                    .value_name("COMMAND")
                    .num_args(1..)
                    .trailing_var_arg(true)
                    .value_parser(clap::value_parser!(OsString)),
            );
        let mut command = observe_help(command);
        let mut matches = command.try_get_matches_from_mut(
            std::iter::once(std::ffi::OsStr::new(namespace))
                .chain(arguments.iter().map(OsString::as_os_str)),
        )?;
        let color_is_explicit = matches.value_source("color") == Some(ValueSource::CommandLine);
        let NamespaceOptions {
            process,
            input,
            config,
        } = NamespaceOptions::from_arg_matches(&matches)?;
        if (global.log_file.is_some() && process.no_log_file)
            || (global.no_log_file && process.log_file.is_some())
        {
            return Err(command.error(
                clap::error::ErrorKind::ArgumentConflict,
                "--log-file conflicts with --no-log-file",
            ));
        }
        if color_is_explicit {
            global.color = process.color;
        }
        global.log_file = process.log_file.or(global.log_file.take());
        global.no_log_file |= process.no_log_file;
        global.quiet |= process.quiet;
        global.verbose = global.verbose.saturating_add(process.verbose);
        Ok(Self {
            namespace: namespace.to_owned(),
            config,
            input,
            help: help_flag(&matches),
            command_args: matches
                .remove_many::<OsString>("command_args")
                .into_iter()
                .flatten()
                .collect(),
        })
    }
}

pub(in crate::cli) fn run(
    args: ExtensionArgs,
    mode: InputMode,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let resources = crate::cli::config::build_resources_with_fonts(
        mode,
        &crate::cli::CompilerFontArgs::default(),
    )?;
    let loaded = match crate::cli::config::load_optional_config(
        &args.config,
        &TypstPackageArgs::default(),
        mode.scope(),
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    ) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return Err(unrecognized_subcommand(&args.namespace)),
        Err(error) => return Err(error),
    };
    let config = std::sync::Arc::new(loaded.into_config());
    let modules = tola_build::modules::select_command_modules(
        config.get_root(),
        config.vendor().path.as_deref(),
        config.modules(),
        mode,
        &cancellation.token(),
    )?;
    check_namespaces(&modules)?;
    let module = modules
        .get(&args.namespace)
        .with_context(|| format!("no site module declares `{}`", args.namespace))?;
    if args.command_args.is_empty() || args.help.is_some() {
        let signatures = command_signatures(
            module,
            module.commands.keys().map(String::as_str),
            config.get_root(),
            config.config_path(),
            mode.scope(),
        )?;
        return output.write_stdout(namespace_help(
            module,
            &signatures,
            output.terminal().stdout_uses_color(),
            args.help == Some(HelpLength::Long),
        ));
    }
    let name = args.command_args[0]
        .to_str()
        .context("module command names must be UTF-8")?;
    let tail = &args.command_args[1..];
    if name == "init" && module.initializer.is_some() {
        return run_initializer(module, tail, &config, mode, output, cancellation);
    }
    let Some(declared) = module.commands.get(name) else {
        let command =
            namespace_command(module, &BTreeMap::new()).color(if output.terminal().uses_color() {
                clap::ColorChoice::Always
            } else {
                clap::ColorChoice::Never
            });
        let error = command
            .try_get_matches_from([module.name.as_str(), name])
            .expect_err("an undeclared module command is rejected");
        return Err(error.into());
    };
    let action = declared.action()?;
    if is_help(tail) && !matches!(&action, CommandAction::Execute(Action::Argv(_))) {
        let color = output.terminal().stdout_uses_color();
        let long = tail[0] == "--help";
        if matches!(&action, CommandAction::Execute(Action::Nu { .. })) {
            let mut signatures = command_signatures(
                module,
                std::iter::once(name),
                config.get_root(),
                config.config_path(),
                mode.scope(),
            )?;
            if let Some(signature) = signatures.remove(name).transpose()? {
                return output.write_stdout(render_help(
                    signature_command(module, name, signature),
                    color,
                    long,
                ));
            }
        }
        return output.write_stdout(command_help(module, name, declared, color, long));
    }
    match &action {
        CommandAction::Execute(Action::Nu { path, args }) => {
            let path = tola_module::module_set::validate_module_path(
                config.get_root(),
                &module.name,
                &module.root,
                path,
                false,
            )?;
            let mut arguments = args.clone();
            arguments.extend(super::nu::utf8_arguments(tail)?);
            let diagnostics = super::nu::run_action(
                &path,
                &arguments,
                &module.name,
                name,
                config,
                resources,
                cancellation,
            )?;
            output.diagnostics(&diagnostics)?;
            Ok(())
        }
        CommandAction::Execute(Action::Argv(argv)) => {
            // This command receives live disk context, not a host-provided text snapshot.
            // Do not turn report source IDs into paths or claim editor locations for it.
            let reports = ReportChannel::new(
                format!("command:{}.{name}", module.name),
                ReportSources::new(),
            )?;
            let (command, _options) = process_command(
                argv,
                tail,
                config.get_root(),
                config.config_path(),
                module,
                mode.scope(),
            )?;
            let command = command
                .env("TOLA_CONTENT_ROOT", config.build().content_dir.as_os_str())
                .env(REPORT_PATH_ENV, reports.path());
            let completion = crate::cli::external::run(command, cancellation);
            cancellation.token().ensure_active()?;
            complete_report(completion, reports, output)
        }
        CommandAction::Query { query, projection } => {
            let modules = tola_build::modules::load_modules(
                config.get_root(),
                config.vendor().path.as_deref(),
                config.modules(),
                mode,
                &cancellation.token(),
            )?;
            let module = modules
                .get(&args.namespace)
                .context("module declarations changed; run the command again")?;
            ensure!(
                module.commands.get(name) == Some(declared),
                "module command changed; run the command again"
            );
            let query_declaration = module
                .queries
                .get(query)
                .context("module query changed; run the command again")?;
            super::query::run_declared(super::query::DeclaredQuery {
                config,
                module,
                query: query_declaration,
                name: &format!("{}.{}", module.name, query),
                usage: &format!("tola {} {name}", module.name),
                projection: (!projection.is_empty()).then_some(projection.as_slice()),
                tail,
                resources,
                output,
                cancellation,
            })
        }
        CommandAction::Skill { path } => {
            let matches = help::skill_command(module, name)
                .try_get_matches_from(
                    std::iter::once(OsString::from(name)).chain(tail.iter().cloned()),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let args = crate::cli::args::SkillArgs::from_arg_matches(&matches)?;
            let path = module.root.join(path);
            let canonical = path.canonicalize().with_context(|| {
                format!("Tola could not read the module skill at {}", path.display())
            })?;
            ensure!(
                canonical.starts_with(&module.root) && canonical.is_file(),
                "module skill `{}` is outside its module or not a file",
                path.display()
            );
            let text = std::fs::read_to_string(&canonical).with_context(|| {
                format!(
                    "Tola could not read the module skill at {}",
                    canonical.display()
                )
            })?;
            super::skill::run(
                &text,
                &format!("tola {} {name}", module.name),
                &args,
                output,
                cancellation,
            )
        }
    }
}

fn unrecognized_subcommand(namespace: &str) -> anyhow::Error {
    let mut command = Cli::command();
    command
        .error(
            clap::error::ErrorKind::InvalidSubcommand,
            format!("unrecognized subcommand `{namespace}`"),
        )
        .into()
}

pub(in crate::cli) fn check_namespaces<T>(modules: &BTreeMap<String, T>) -> Result<()> {
    let builtin = Cli::command();
    for command in builtin.get_subcommands() {
        ensure!(
            !modules.contains_key(command.get_name()),
            "`{}` is a built-in Tola command, so a module cannot use that name",
            command.get_name(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
