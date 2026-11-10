//! Commands explicitly declared by the site's local modules.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::{CommandFactory, FromArgMatches};
use tola_build::InputScope;
use tola_build::modules::CommandAction;
use tola_build::report::{REPORT_PATH_ENV, ReportChannel, ReportSources};

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
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

struct Invocation {
    namespace: String,
    config: ConfigFileArgs,
    arguments: Vec<OsString>,
}

impl Invocation {
    fn parse(mut arguments: Vec<OsString>) -> Result<Self> {
        ensure!(!arguments.is_empty(), "a module namespace is required");
        let namespace = arguments
            .remove(0)
            .into_string()
            .map_err(|_| anyhow::anyhow!("module namespaces must be UTF-8"))?;
        let mut config = ConfigFileArgs::default();
        loop {
            let path = if arguments
                .first()
                .is_some_and(|value| value == "--config" || value == "-C")
            {
                arguments.remove(0);
                ensure!(!arguments.is_empty(), "--config requires a path");
                PathBuf::from(arguments.remove(0))
            } else if let Some(path) = arguments
                .first()
                .and_then(|value| value.to_str())
                .and_then(|value| value.strip_prefix("--config="))
            {
                let path = PathBuf::from(path);
                arguments.remove(0);
                path
            } else {
                break;
            };
            ensure!(!path.as_os_str().is_empty(), "--config requires a path");
            ensure!(config.path.is_none(), "--config may be supplied only once");
            config.path = Some(path);
        }
        Ok(Self {
            namespace,
            config,
            arguments,
        })
    }
}

pub(in crate::cli) fn run(
    arguments: Vec<OsString>,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let invocation = Invocation::parse(arguments)?;
    let request = super::SiteRequest::new(
        invocation.config,
        TypstPackageArgs::default(),
        scope,
        crate::cli::BuildOverrideArgs::default(),
        crate::cli::CompilerFontArgs::default(),
        output,
        cancellation,
    )?;
    let loaded = request.load_config(&crate::config::ConfigOverrides::default())?;
    let config = std::sync::Arc::new(loaded.into_config());
    let modules =
        tola_build::modules::resolve_command_modules(config.get_root(), config.modules(), scope)?;
    check_namespaces(&modules)?;
    let module = modules
        .get(&invocation.namespace)
        .with_context(|| format!("no site module declares `{}`", invocation.namespace))?;
    if invocation.arguments.is_empty() || is_help(&invocation.arguments) {
        let signatures = command_signatures(
            module,
            module.commands.keys().map(String::as_str),
            config.get_root(),
            config.config_path(),
            scope,
        )?;
        return output.write_stdout(namespace_help(
            module,
            &signatures,
            output.terminal().stdout_uses_color(),
            invocation
                .arguments
                .first()
                .is_some_and(|argument| argument == "--help"),
        ));
    }
    let name = invocation.arguments[0]
        .to_str()
        .context("module command names must be UTF-8")?;
    let tail = &invocation.arguments[1..];
    if name == "init" && module.initializer.is_some() {
        if !is_help(tail)
            && matches!(
                config.modules()[&invocation.namespace].source,
                tola_build::modules::ModuleSource::Remote { .. }
            )
        {
            bail!(
                "initialization from a remote module is not supported; use a local module with `tola init --from`"
            );
        }
        return run_initializer(module, tail, scope, output, cancellation);
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
    if is_help(tail) {
        let signatures = command_signatures(
            module,
            std::iter::once(name),
            config.get_root(),
            config.config_path(),
            scope,
        )?;
        if let Some(signature) = signatures.get(name) {
            return output.write_stdout(render_help(
                signature_command(module, name, signature.clone()),
                output.terminal().stdout_uses_color(),
                tail[0] == "--help",
            ));
        }
        return output.write_stdout(command_help(
            module,
            name,
            declared,
            output.terminal().stdout_uses_color(),
            tail[0] == "--help",
        ));
    }
    match &declared.action {
        CommandAction::Argv { argv } => {
            // This command receives live disk context, not a host-provided text snapshot.
            // Do not turn report source IDs into paths or claim editor locations for it.
            let reports = ReportChannel::new(
                format!("command:{}.{name}", module.name),
                ReportSources::new(),
            )?;
            let command = process_command(
                argv,
                tail,
                config.get_root(),
                config.config_path(),
                module,
                scope,
            )?
            .env("TOLA_CONTENT_ROOT", config.build().content_dir.as_os_str())
            .env(REPORT_PATH_ENV, reports.path());
            let completion = crate::cli::external::run(command, cancellation);
            cancellation.token().ensure_active()?;
            complete_report(completion, reports, output)
        }
        CommandAction::Query { query, projection } => {
            let modules =
                tola_build::modules::resolve_modules(config.get_root(), config.modules(), scope)?;
            let module = modules
                .get(&invocation.namespace)
                .context("module declarations changed; run the command again")?;
            ensure!(
                module.commands.get(name) == Some(declared),
                "module command changed; run the command again"
            );
            let query_declaration = module
                .queries
                .get(query)
                .context("module query changed; run the command again")?;
            super::query::run_declared(
                config,
                module,
                query_declaration,
                &format!("{}.{}", module.name, query),
                &format!("tola {} {name}", module.name),
                (!projection.is_empty()).then_some(projection.as_slice()),
                tail,
                request.resources,
                output,
                cancellation,
            )
        }
        CommandAction::Skill => {
            let matches = help::skill_command(module, name)
                .try_get_matches_from(
                    std::iter::once(OsString::from(name)).chain(tail.iter().cloned()),
                )
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let args = crate::cli::args::SkillArgs::from_arg_matches(&matches)?;
            let path = module
                .skill
                .as_deref()
                .context("module skill changed; run the command again")?;
            let canonical = path.canonicalize().with_context(|| {
                format!("Tola could not read the module skill at {}", path.display())
            })?;
            ensure!(
                canonical.starts_with(&module.root) && canonical.is_file(),
                "module skill `{}` is outside its module or not a file",
                path.display()
            );
            let text = std::fs::read_to_string(&canonical).with_context(|| {
                format!("Tola could not read the module skill at {}", canonical.display())
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

pub(super) fn check_namespaces<T>(modules: &BTreeMap<String, T>) -> Result<()> {
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
