//! Best-effort discovery and rendering for static CLI help.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, ArgAction, ColorChoice, CommandFactory};
use std::collections::BTreeMap;
use tola_build::InputScope;
use tola_build::modules::manifest::ModuleDeclarations;

#[derive(PartialEq)]
enum Page {
    Top,
    Queries,
    Nu,
}

struct Request {
    page: Page,
    config: Option<PathBuf>,
    scope: InputScope,
    long: bool,
    color: ColorChoice,
}

// Parse the same arguments as the CLI, with help recorded instead of exiting. Root help also
// accepts a config path, and query help does not require a query name.
fn request(args: &[OsString]) -> Option<Request> {
    let command = super::Cli::command()
        .arg_required_else_help(false)
        .subcommand_required(false)
        .disable_help_flag(true)
        .arg(
            Arg::new("site_help_short")
                .short('h')
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("site_help_long")
                .long("help")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("site_help_config")
                .short('C')
                .long("config")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .mut_subcommand("query", |query| {
            query
                .disable_help_flag(true)
                .mut_arg("target", |target| target.required(false))
        })
        .mut_subcommand("nu", |nu| {
            nu.disable_help_flag(true).mut_arg("invocation", |entry| {
                entry.required_unless_present_any([
                    "lsp",
                    "lsp_worker",
                    "site_help_short",
                    "site_help_long",
                ])
            })
        });
    let matches = command.try_get_matches_from(args).ok()?;
    let long = matches.get_flag("site_help_long");
    let help = long || matches.get_flag("site_help_short");
    let (page, config) = match matches.subcommand() {
        None if help || args.len() == 1 || matches.contains_id("site_help_config") => (
            Page::Top,
            matches.get_one::<PathBuf>("site_help_config").cloned(),
        ),
        Some(("query", query)) if query.get_one::<String>("target").is_none() => {
            if matches.contains_id("site_help_config") && query.contains_id("path") {
                return None;
            }
            (
                Page::Queries,
                query
                    .get_one::<PathBuf>("path")
                    .cloned()
                    .or_else(|| matches.get_one::<PathBuf>("site_help_config").cloned()),
            )
        }
        Some(("nu", nu)) if help && nu.get_many::<OsString>("invocation").is_none() => {
            if matches.contains_id("site_help_config") && nu.contains_id("path") {
                return None;
            }
            (
                Page::Nu,
                nu.get_one::<PathBuf>("path")
                    .cloned()
                    .or_else(|| matches.get_one::<PathBuf>("site_help_config").cloned())
                    .or_else(|| std::env::var_os("TOLA_CONFIG_PATH").map(PathBuf::from)),
            )
        }
        _ => return None,
    };
    let scope = if matches.get_flag("pure") {
        InputScope::Pure
    } else if matches.get_flag("offline") {
        InputScope::Offline
    } else if page == Page::Nu {
        let inherited = std::env::var("TOLA_INPUT_SCOPE").ok();
        InputScope::parse(inherited.as_deref().unwrap_or("online"))?
    } else {
        InputScope::Online
    };
    let color = *matches.get_one::<ColorChoice>("color")?;
    Some(Request {
        page,
        config,
        scope,
        long,
        color,
    })
}

fn discover(request: &Request) -> anyhow::Result<BTreeMap<String, ModuleDeclarations>> {
    let source = tola_build::config::ConfigSource::load(request.config.as_deref(), request.scope)?;
    let (core, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let schema = core.into_schema();
    let mut modules = tola_build::modules::manifest::read_declarations(
        source.root(),
        &schema.modules,
        request.scope,
    )?;
    let builtins = super::Cli::command();
    anyhow::ensure!(
        builtins
            .get_subcommands()
            .all(|c| !modules.contains_key(c.get_name())),
        "reserved module namespace"
    );
    if request.page == Page::Top
        && let Ok(commands) = tola_build::modules::resolve_command_modules(
            source.root(),
            &schema.modules,
            request.scope,
        )
    {
        let declared = commands.values().flat_map(|module| {
            module
                .commands
                .keys()
                .map(move |name| (module, name.as_str()))
        });
        if let Ok(signatures) = super::commands::extensions::module_command_signatures(
            declared,
            source.root(),
            source.path(),
            request.scope,
        ) {
            for ((module, name), signature) in signatures {
                if let (Some(module), Ok(signature)) = (modules.get_mut(&module), signature)
                    && let Some(help) = module.commands.get_mut(&name)
                {
                    *help = signature.description;
                }
            }
        }
    }
    Ok(modules)
}

fn append_site_help(
    help: &mut String,
    page: &Page,
    modules: &BTreeMap<String, ModuleDeclarations>,
) {
    let rows: Vec<(String, &str)> = match page {
        Page::Top => modules
            .iter()
            .filter(|(_, m)| !m.commands.is_empty())
            .flat_map(|(n, m)| {
                std::iter::once((n.clone(), m.summary.as_str())).chain(
                    m.commands
                        .iter()
                        .map(move |(command, help)| (format!("{n} {command}"), help.as_str())),
                )
            })
            .collect(),
        Page::Queries => modules
            .iter()
            .flat_map(|(n, m)| m.queries.iter().map(move |q| (format!("{n}.{q}"), "")))
            .collect(),
        Page::Nu => modules
            .iter()
            .filter(|(_, module)| module.nu_entry.is_some())
            .map(|(name, module)| (format!("@tola-modules/{name}"), module.summary.as_str()))
            .collect(),
    };
    if rows.is_empty() {
        return;
    }
    help.push_str(match page {
        Page::Top => "\nSite commands:\n",
        Page::Queries => "\nSite queries:\n",
        Page::Nu => "\nSite Nu modules (import with `use tola-modules ALIAS`):\n",
    });
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (name, summary) in rows {
        if summary.is_empty() {
            help.push_str(&format!("  {name}\n"));
        } else {
            help.push_str(&format!(
                "  {name:width$}  {}\n",
                summary.lines().next().unwrap_or("")
            ));
        }
    }
}

pub(super) fn for_process() -> Option<ExitCode> {
    let request = request(&std::env::args_os().collect::<Vec<_>>())?;
    let color =
        if crate::terminal::color::enabled(request.color, crate::terminal::color::Stream::Stdout) {
            ColorChoice::Always
        } else {
            ColorChoice::Never
        };
    let mut command = super::Cli::command().color(color);
    let target = match request.page {
        Page::Top => &mut command,
        Page::Queries => {
            let query = command.find_subcommand_mut("query")?;
            *query = query.clone().bin_name("tola query").color(color);
            query
        }
        Page::Nu => {
            let nu = command.find_subcommand_mut("nu")?;
            *nu = nu.clone().bin_name("tola nu").color(color);
            nu
        }
    };
    let rendered = if request.long {
        target.render_long_help()
    } else {
        target.render_help()
    };
    let mut help = if color == ColorChoice::Always {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    };
    if !help.ends_with('\n') {
        help.push('\n');
    }
    if request.page == Page::Nu {
        help.push_str(
            "\nRoot query: `use tola` then `tola query`, or `use tola query` then `query`.\n",
        );
        help.push_str("\nBuilt-in Nu modules (import with `use tola NAME`):\n");
        for (name, summary) in tola_nu::BUILTIN_MODULES {
            help.push_str(&format!("  @tola-scripts/{name:<16} {summary}\n"));
        }
        help.push_str("\nRun a module with `tola nu @tola-scripts/NAME --help` when it exports main.\n");
    }
    if let Ok(modules) = discover(&request) {
        append_site_help(&mut help, &request.page, &modules);
    }
    let result = std::io::stdout().lock().write_all(help.as_bytes());
    Some(
        if result.is_ok()
            || result
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}
