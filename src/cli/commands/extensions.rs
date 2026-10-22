//! Commands explicitly declared by the site's local modules.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, CommandFactory, FromArgMatches};
use tola_build::InputScope;
use tola_build::modules::{CommandAction, ModuleCommands};
use tola_build::report::{REPORT_PATH_ENV, ReportChannel, ReportSources};
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_subprocess::Command;

use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::cli::{Cli, ConfigFileArgs, TypstPackageArgs};

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
    }
}

fn complete_report(
    completion: Result<()>,
    reports: ReportChannel,
    output: &CommandOutput,
) -> Result<()> {
    let diagnostics = match reports.read() {
        Ok(diagnostics) => diagnostics.unwrap_or_default(),
        Err(error) => {
            let message = "invalid module command diagnostic report";
            let diagnostic =
                Diagnostic::new(crate::codes::command::FAILED, Severity::Error, message)
                    .with_help("Ask the module author to check the report format and use only source locations supplied by Tola");
            let error = anyhow::Error::new(error).context(message);
            return match completion {
                Ok(()) => Err(error.context(DiagnosticError::new(message, vec![diagnostic]))),
                Err(process_error) => Err(process_error
                    .context(error)
                    .context(DiagnosticError::new(message, vec![diagnostic]))),
            };
        }
    };
    if let Err(error) = completion {
        return if diagnostics.is_empty() {
            Err(error)
        } else {
            let message = error.to_string();
            Err(error.context(DiagnosticError::new(message, diagnostics)))
        };
    }
    if super::holds_error(&diagnostics) {
        return Err(DiagnosticError::new(
            "a reported error prevents this module command from succeeding",
            diagnostics,
        )
        .into());
    }
    output.diagnostics(&diagnostics)?;
    Ok(())
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

fn is_help(arguments: &[OsString]) -> bool {
    arguments.len() == 1 && (arguments[0] == "--help" || arguments[0] == "-h")
}

fn namespace_command(
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

fn namespace_help(
    module: &ModuleCommands,
    signatures: &BTreeMap<String, tola_nu::Signature>,
    use_color: bool,
    long: bool,
) -> String {
    render_help(namespace_command(module, signatures), use_color, long)
}

fn command_signatures<'a>(
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
    let signatures = super::nu::command_signatures(&invocations, project, config, scope);
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

fn render_help(mut command: clap::Command, use_color: bool, long: bool) -> String {
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

fn run_initializer(
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
    super::init::run(&args, scope, output, cancellation)
}

fn command_help(
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
        CommandAction::Query { .. } => super::query::QueryOptions::augment_args(command),
        CommandAction::Argv { .. } => command.arg(
            clap::Arg::new("ARGS")
                .num_args(0..)
                .allow_hyphen_values(true)
                .help("Arguments passed to the command"),
        ),
    };
    render_help(command, use_color, long)
}

fn signature_command(
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

fn expand(argument: &str, project: &Path, module: &Path, executable: &Path) -> OsString {
    tola_build::modules::expand_argument(
        argument,
        &[
            ("{project}", project.as_os_str()),
            ("{module}", module.as_os_str()),
            ("{tola}", executable.as_os_str()),
        ],
    )
}

fn process_command(
    argv: &[String],
    tail: &[OsString],
    project: &Path,
    config: &Path,
    module: &ModuleCommands,
    scope: InputScope,
) -> Result<Command> {
    let Some(program) = argv.first() else {
        bail!("declared command has no executable");
    };
    let executable = crate::cli::external::current_executable()?;
    let program = expand(program, project, &module.root, &executable);
    ensure!(
        !program.is_empty(),
        "declared command has an empty executable"
    );
    let command = Command::new(program)
        .args(
            argv[1..]
                .iter()
                .map(|arg| expand(arg, project, &module.root, &executable)),
        )
        .args(tail.iter().cloned())
        .cwd(project)
        .env("TOLA_PROJECT_ROOT", project.as_os_str())
        .env("TOLA_MODULE_ROOT", module.root.as_os_str())
        .env("TOLA_MODULE_NAME", module.name.as_str())
        .env("TOLA_MODULE_MANIFEST", module.manifest.as_os_str())
        .env_remove("TOLA_MODULE_OPTIONS")
        .env("TOLA_CONFIG_PATH", config.as_os_str())
        .env("TOLA_INPUT_SCOPE", scope.as_str())
        .env("TOLA_EXECUTABLE", executable);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::external::{CommandExit, run as run_process};

    fn report_channel(severity: &str) -> ReportChannel {
        let reports = ReportChannel::new("command:catalog.scan", ReportSources::new()).unwrap();
        std::fs::write(
            reports.path(),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "diagnostics": [{"severity": severity, "message": "catalog issue", "code": "rule"}],
            }))
            .unwrap(),
        )
        .unwrap();
        reports
    }

    #[test]
    fn report_severity_controls_success() {
        let output = super::super::testing::buffered_output();
        complete_report(Ok(()), report_channel("warning"), &output).unwrap();
        let error = complete_report(Ok(()), report_channel("error"), &output).unwrap_err();
        let diagnostics = tola_diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(
            diagnostics[0].producer.as_ref().unwrap().name.as_deref(),
            Some("command:catalog.scan"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_process_status_survives_reports() {
        use std::os::unix::process::ExitStatusExt;

        let output = super::super::testing::buffered_output();
        for invalid in [false, true] {
            let reports = report_channel("warning");
            if invalid {
                std::fs::write(reports.path(), b"partial report").unwrap();
            }
            let completion = Err(CommandExit(std::process::ExitStatus::from_raw(37 << 8)).into());
            let error = complete_report(completion, reports, &output).unwrap_err();
            assert_eq!(
                error.downcast_ref::<CommandExit>().unwrap().0.code(),
                Some(37)
            );
            let diagnostics = tola_diagnostic::attached(&error).unwrap();
            assert_eq!(diagnostics.len(), 1);
            if invalid {
                assert_eq!(diagnostics[0].severity, Severity::Error);
                assert!(
                    diagnostics[0]
                        .message
                        .contains("invalid module command diagnostic report")
                );
            } else {
                assert_eq!(diagnostics[0].severity, Severity::Warning);
            }
        }
    }

    fn module() -> ModuleCommands {
        ModuleCommands {
            name: "forest".into(),
            root: PathBuf::from("/module with spaces"),
            manifest: PathBuf::from("/module with spaces/tola-module.toml"),
            summary: "Forest author tools".into(),
            commands: BTreeMap::from([(
                "edit".into(),
                tola_build::modules::Command {
                    help: "Edit a tree\nLonger instructions.".into(),
                    action: CommandAction::Argv {
                        argv: vec!["missing-tool".into()],
                    },
                },
            )]),
            initializer: None,
        }
    }

    #[test]
    fn static_help_needs_no_executable() {
        let module = module();
        let help = namespace_help(&module, &BTreeMap::new(), false, true);
        assert!(help.contains("forest"));
        assert!(help.contains("Edit a tree"));
        assert!(
            command_help(&module, "edit", &module.commands["edit"], false, true)
                .contains("Longer instructions.")
        );
        let query = tola_build::modules::Command {
            help: "Find trees".into(),
            action: CommandAction::Query {
                query: "nodes".into(),
                projection: vec![],
            },
        };
        let query_help = command_help(&module, "find", &query, false, true);
        assert!(query_help.contains("--output"));
    }

    #[test]
    fn namespace_config_stops_declared_command() {
        let invocation = Invocation::parse(
            [
                "forest",
                "--config",
                "a site/tola.toml",
                "edit",
                "--config",
                "child.toml",
            ]
            .map(OsString::from)
            .to_vec(),
        )
        .unwrap();
        assert_eq!(
            invocation.config.path.as_deref(),
            Some(Path::new("a site/tola.toml"))
        );
        assert_eq!(
            invocation.arguments,
            ["edit", "--config", "child.toml"].map(OsString::from)
        );
        let equals = Invocation::parse(
            ["forest", "--config=a site/tola.toml", "--help"]
                .map(OsString::from)
                .to_vec(),
        )
        .unwrap();
        assert_eq!(equals.config.path, invocation.config.path);
    }

    #[test]
    fn declared_argv_paths_arguments_separator() {
        let module = module();
        let project = Path::new("/project with spaces");
        let command = process_command(
            &[
                "deno".into(),
                "{module}/tools/cli.ts".into(),
                "--project={project}".into(),
            ],
            &["a title".into(), "--".into(), "$(literal)".into()],
            project,
            &project.join("tola.toml"),
            &module,
            InputScope::Online,
        )
        .unwrap();
        assert_eq!(command.program(), "deno");
        assert_eq!(
            expand(
                "{module}/tools/cli.ts",
                project,
                &module.root,
                Path::new("/tola")
            ),
            "/module with spaces/tools/cli.ts"
        );
        assert_eq!(
            expand(
                "--project={project}",
                project,
                &module.root,
                Path::new("/tola")
            ),
            "--project=/project with spaces"
        );
    }

    #[test]
    fn builtins_cannot_be_module_namespaces() {
        let mut module = module();
        module.name = "build".into();
        assert!(check_namespaces(&BTreeMap::from([("build".into(), module)])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn external_argv_context_exit_status() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path();
        let mut module = module();
        module.root = project.join("module with spaces");
        module.name = "notes".into();
        std::fs::create_dir(&module.root).unwrap();
        let record = project.join("arguments.txt");
        let script = module.root.join("record.sh");
        std::fs::write(
            &script,
            r#"record="$1"
shift
printf '%s\n' "$@" "$TOLA_PROJECT_ROOT" "$TOLA_MODULE_ROOT" "$TOLA_CONFIG_PATH" "$TOLA_EXECUTABLE" "$TOLA_INPUT_SCOPE" "$TOLA_MODULE_NAME" > "$record"
exit 37
"#,
        )
        .unwrap();
        let argv = vec![
            "sh".into(),
            "{module}/record.sh".into(),
            "{project}/arguments.txt".into(),
        ];
        for (scope, expected_scope) in [
            (InputScope::Online, "online"),
            (InputScope::Offline, "offline"),
            (InputScope::Pure, "pure"),
        ] {
            let command = process_command(
                &argv,
                &[
                    "a title with spaces".into(),
                    "--".into(),
                    "$(literal)".into(),
                ],
                project,
                &project.join("custom config.toml"),
                &module,
                scope,
            )
            .unwrap();
            let error = run_process(command, &Cancellation::default()).unwrap_err();
            assert_eq!(
                error.downcast_ref::<CommandExit>().unwrap().0.code(),
                Some(37)
            );
            let recorded = std::fs::read_to_string(&record).unwrap();
            let lines = recorded.lines().collect::<Vec<_>>();
            assert_eq!(&lines[..3], ["a title with spaces", "--", "$(literal)"]);
            assert_eq!(Path::new(lines[3]), project);
            assert_eq!(Path::new(lines[4]), module.root);
            assert_eq!(Path::new(lines[5]), project.join("custom config.toml"));
            assert_eq!(Path::new(lines[6]), std::env::current_exe().unwrap());
            assert_eq!(lines[7], expected_scope);
            assert_eq!(lines[8], "notes");
        }
    }

    #[test]
    fn unavailable_declared_executable_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("tool-that-is-not-installed");
        let command = process_command(
            &[program.to_string_lossy().into_owned()],
            &[],
            directory.path(),
            &directory.path().join("tola.toml"),
            &module(),
            InputScope::Online,
        )
        .unwrap();
        let error = run_process(command, &Cancellation::default()).unwrap_err();
        assert!(error.to_string().contains("Tola could not run"), "{error}");
        assert!(error.downcast_ref::<CommandExit>().is_none());
    }
}
