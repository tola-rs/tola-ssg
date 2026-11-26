//! Best-effort discovery and rendering for static CLI help.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{
    Arg, ArgAction, ArgMatches, ColorChoice, CommandFactory, FromArgMatches, builder::StyledStr,
};
use tola_input::InputScope;
use tola_module::manifest::ModuleDeclarations;

use super::args::{Cli, ScopeArgs};
use crate::terminal::color::{Stream, enabled};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Page {
    Top,
    Queries,
    Nu,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum HelpLength {
    Short,
    Long,
}

/// Record help flags when the caller needs their matches instead of clap's rendered page.
pub(super) fn observe_help(command: clap::Command) -> clap::Command {
    command
        .disable_help_flag(true)
        .arg(Arg::new("short_help").short('h').action(ArgAction::SetTrue))
        .arg(
            Arg::new("long_help")
                .long("help")
                .action(ArgAction::SetTrue),
        )
}

pub(super) fn help_flag(matches: &ArgMatches) -> Option<HelpLength> {
    if matches.try_get_one::<bool>("long_help").ok().flatten() == Some(&true) {
        Some(HelpLength::Long)
    } else if matches.try_get_one::<bool>("short_help").ok().flatten() == Some(&true) {
        Some(HelpLength::Short)
    } else {
        None
    }
}

struct SiteHelp {
    page: Page,
    config: Option<PathBuf>,
    scope: InputScope,
}

impl SiteHelp {
    const ROOT: Self = Self {
        page: Page::Top,
        config: None,
        scope: InputScope::Offline,
    };
}

fn site_help(matches: &ArgMatches) -> Option<SiteHelp> {
    if help_flag(matches).is_some() {
        return Some(SiteHelp::ROOT);
    }
    let (name, leaf) = matches.subcommand()?;
    let page = match name {
        "query" => Page::Queries,
        "nu" if help_flag(leaf) == Some(HelpLength::Long) && !leaf.get_flag("no_site") => Page::Nu,
        _ => return None,
    };
    help_flag(leaf)?;
    let mut config = leaf.get_one::<PathBuf>("path").cloned();
    let mut scope = ScopeArgs::from_arg_matches(leaf).ok()?.scope();
    if page == Page::Nu
        && let Some(call) = tola_build::context::Context::read().ok().flatten()
    {
        if config.is_none() {
            config = call.site.map(|site| site.config_path);
        }
        if scope == InputScope::Online {
            scope = call
                .input_scope
                .as_deref()
                .and_then(InputScope::parse)
                .unwrap_or(scope);
        }
    }
    Some(SiteHelp {
        page,
        config,
        scope,
    })
}

/// Clap's native help result has rendered text but no matches. Read the request with the same
/// argument definitions, allowing a missing target such as `tola query --help`.
fn help_matches(arguments: &[OsString]) -> Option<ArgMatches> {
    fn observe_tree(command: clap::Command) -> clap::Command {
        observe_help(command).mut_subcommands(observe_tree)
    }

    observe_tree(Cli::command())
        .ignore_errors(true)
        .try_get_matches_from(arguments)
        .ok()
}

fn discover(
    config: Option<&Path>,
    scope: InputScope,
) -> anyhow::Result<BTreeMap<String, ModuleDeclarations>> {
    let scope = if scope == InputScope::Pure {
        scope
    } else {
        InputScope::Offline
    };
    let source = tola_build::config::ConfigSource::load(config, scope)?;
    let (core, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    core.validate_source_paths()?;
    let mut schema = core.into_schema();
    schema.vendor.normalize(source.root());
    let mut modules = tola_build::modules::read_available_declarations(
        source.root(),
        schema.vendor.path.as_deref(),
        &schema.modules,
        scope,
    )?;
    let builtins = Cli::command();
    modules.retain(|name, _| builtins.find_subcommand(name).is_none());
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
                m.commands
                    .iter()
                    .map(move |(command, help)| (format!("{n} {command}"), help.as_str()))
            })
            .collect(),
        Page::Queries => modules
            .iter()
            .flat_map(|(n, m)| m.queries.iter().map(move |q| (format!("{n}.{q}"), "")))
            .collect(),
        Page::Nu => modules
            .iter()
            .filter(|(_, module)| module.nu_entry.is_some())
            .map(|(name, _)| (format!("use tola-modules {name}"), ""))
            .collect(),
    };
    if rows.is_empty() {
        return;
    }
    help.push_str(match page {
        Page::Top => "\nSite commands:\n",
        Page::Queries => "\nSite queries:\n",
        Page::Nu => "\nSite Nu libraries:\n",
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

pub(super) fn root(help: StyledStr) -> ExitCode {
    emit_page(help, ColorChoice::Auto, Some(SiteHelp::ROOT), false)
}

pub(super) fn render(arguments: &[OsString], error: clap::Error) -> ExitCode {
    let matches = help_matches(arguments);
    let mut color = ColorChoice::Auto;
    let mut current = matches.as_ref();
    while let Some(matches) = current {
        if let Ok(Some(value)) = matches.try_get_one::<ColorChoice>("color") {
            color = *value;
        }
        current = matches.subcommand().map(|(_, child)| child);
    }
    let nu_long = matches
        .as_ref()
        .and_then(|matches| matches.subcommand_matches("nu"))
        .is_some_and(|leaf| help_flag(leaf) == Some(HelpLength::Long));
    let request = matches.as_ref().and_then(site_help);
    emit_page(error.render(), color, request, nu_long)
}

fn emit_page(
    rendered: StyledStr,
    color: ColorChoice,
    request: Option<SiteHelp>,
    nu_long: bool,
) -> ExitCode {
    let mut help = if enabled(color, Stream::Stdout) {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    };
    if !help.ends_with('\n') {
        help.push('\n');
    }
    if nu_long {
        append_builtin_nu(&mut help);
    }
    if let Some(request) = request
        && let Ok(modules) = discover(request.config.as_deref(), request.scope)
    {
        append_site_help(&mut help, &request.page, &modules);
    }
    emit(help)
}

fn append_builtin_nu(help: &mut String) {
    help.push_str("\nBuilt-in Nu libraries:\n");
    let modules = tola_nu::BUILTIN_MODULES
        .iter()
        .chain(super::commands::nu::BUILTIN_MODULES);
    for module in modules.clone() {
        help.push_str(&format!(
            "  use tola {:<12} {}\n",
            module.name, module.summary
        ));
    }
    help.push_str("  use tola syntax-highlight  Highlight source text for terminal previews\n");
    help.push_str("\nBuilt-in scripts:\n");
    for module in modules.filter(|module| module.executable) {
        help.push_str(&format!("  tola nu @tola-scripts/{} --help\n", module.name));
    }
}

fn emit(help: String) -> ExitCode {
    let result = std::io::stdout().lock().write_all(help.as_bytes());
    if result.is_ok()
        || result
            .as_ref()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
    {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(vals: &[&str]) -> Vec<OsString> {
        vals.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_reads_query_config_scope() {
        let matches =
            help_matches(&args(&["tola", "query", "-Csite.toml", "--pure", "-h"])).unwrap();
        let req = site_help(&matches).unwrap();
        assert_eq!(req.page, Page::Queries);
        assert_eq!(req.config, Some(PathBuf::from("site.toml")));
        assert_eq!(req.scope, InputScope::Pure);
    }

    #[test]
    fn nu_help_forwards_script_entry() {
        for flag in ["-h", "--help"] {
            let native = Cli::command()
                .try_get_matches_from(["tola", "nu", "-C", "site.toml", flag])
                .unwrap_err();
            assert_eq!(native.kind(), clap::error::ErrorKind::DisplayHelp);
            let matches = help_matches(&args(&["tola", "nu", "-C", "site.toml", flag])).unwrap();
            assert_eq!(
                site_help(&matches).map(|r| r.page),
                (flag == "--help").then_some(Page::Nu)
            );

            for entry in ["script.nu", "@tola-scripts/image", "@tola-modules/catalog"] {
                let argv = [
                    "tola",
                    "nu",
                    "-C",
                    "site.toml",
                    entry,
                    flag,
                    "--color",
                    "never",
                ];
                let cmd = Cli::command().try_get_matches_from(argv).unwrap();
                let nu = cmd.subcommand_matches("nu").unwrap();
                assert_eq!(
                    nu.get_many::<OsString>("invocation")
                        .unwrap()
                        .cloned()
                        .collect::<Vec<_>>(),
                    args(&argv[4..])
                );
                assert!(site_help(&help_matches(&args(&argv)).unwrap()).is_none());
            }
        }
    }
}
