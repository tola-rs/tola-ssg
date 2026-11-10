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

use super::args::{Cli, InputArgs};
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
    discover: bool,
}

impl SiteHelp {
    const ROOT: Self = Self {
        page: Page::Top,
        config: None,
        scope: InputScope::Online,
        discover: true,
    };
}

fn site_help(matches: &ArgMatches) -> Option<SiteHelp> {
    if help_flag(matches).is_some() {
        return Some(SiteHelp::ROOT);
    }
    let (name, leaf) = matches.subcommand()?;
    let page = match name {
        "query" => Page::Queries,
        "nu" => Page::Nu,
        _ => return None,
    };
    help_flag(leaf)?;
    let mut config = leaf.get_one::<PathBuf>("path").cloned();
    let mut scope = InputArgs::from_arg_matches(leaf).ok()?.scope();
    if page == Page::Nu {
        if config.is_none() {
            config = std::env::var_os("TOLA_CONFIG_PATH").map(PathBuf::from);
        }
        if scope == InputScope::Online {
            scope = std::env::var("TOLA_INPUT_SCOPE")
                .ok()
                .and_then(|value| InputScope::parse(&value))
                .unwrap_or(scope);
        }
    }
    Some(SiteHelp {
        page,
        config,
        scope,
        discover: page != Page::Nu || !leaf.get_flag("no_site"),
    })
}

/// Clap's native help result has rendered text but no matches. Read the request with the same
/// argument definitions, allowing a missing target such as `tola query --help`.
fn help_matches(arguments: &[OsString]) -> Option<ArgMatches> {
    observe_help(Cli::command())
        .mut_subcommand("query", observe_help)
        .mut_subcommand("nu", observe_help)
        .ignore_errors(true)
        .try_get_matches_from(arguments)
        .ok()
}

fn discover(
    page: Page,
    config: Option<&Path>,
    scope: InputScope,
) -> anyhow::Result<BTreeMap<String, ModuleDeclarations>> {
    let source = tola_build::config::ConfigSource::load(config, scope)?;
    let (core, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    core.validate_source_paths()?;
    let mut schema = core.into_schema();
    schema.vendor.normalize(source.root());
    let vendor = schema.vendor.path.as_deref();
    let mode = tola_input::InputMode::new(scope, true);
    let mut modules = tola_build::modules::manifest::read_declarations(
        source.root(),
        vendor,
        &schema.modules,
        mode,
        &Default::default(),
    )?;
    super::commands::extensions::check_namespaces(&modules)?;
    if page == Page::Top
        && let Ok(commands) = tola_build::modules::select_command_modules(
            source.root(),
            vendor,
            &schema.modules,
            mode,
            &Default::default(),
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
            scope,
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
                std::iter::once((n.clone(), "")).chain(
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

pub(super) fn root(help: StyledStr, color: ColorChoice) -> ExitCode {
    emit_page(help, color, Some(SiteHelp::ROOT))
}

pub(super) fn render(arguments: &[OsString], error: clap::Error) -> ExitCode {
    let matches = help_matches(arguments);
    let color = matches
        .as_ref()
        .and_then(|matches| matches.get_one::<ColorChoice>("color"))
        .copied()
        .unwrap_or_else(|| super::argv::color_hint(arguments));
    let request = matches.as_ref().and_then(site_help);
    emit_page(error.render(), color, request)
}

fn emit_page(rendered: StyledStr, color: ColorChoice, request: Option<SiteHelp>) -> ExitCode {
    let mut help = if enabled(color, Stream::Stdout) {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    };
    if !help.ends_with('\n') {
        help.push('\n');
    }
    if let Some(request) = request {
        if request.page == Page::Nu {
            append_builtin_nu(&mut help);
        }
        if request.discover
            && let Ok(modules) = discover(request.page, request.config.as_deref(), request.scope)
        {
            append_site_help(&mut help, &request.page, &modules);
        }
    }
    emit(help)
}

fn append_builtin_nu(help: &mut String) {
    help.push_str("\nFlags before ENTRY apply to Tola. Flags after ENTRY go to the script.\n");
    help.push_str("\nBuilt-in Nu libraries:\n");
    for module in tola_nu::BUILTIN_MODULES {
        help.push_str(&format!(
            "  use tola {:<12} {}\n",
            module.name, module.summary
        ));
    }
    help.push_str("\nBuilt-in scripts:\n");
    for module in tola_nu::BUILTIN_MODULES
        .iter()
        .filter(|module| module.executable)
    {
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

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_reads_the_query_config_and_scope() {
        let matches =
            help_matches(&args(&["tola", "query", "-Csite.toml", "--pure", "-h"])).unwrap();
        let request = site_help(&matches).unwrap();
        assert_eq!(request.page, Page::Queries);
        assert_eq!(request.config, Some(PathBuf::from("site.toml")));
        assert_eq!(request.scope, InputScope::Pure);
    }

    #[test]
    fn nu_help_respects_the_script_entry() {
        for flag in ["-h", "--help"] {
            let native = Cli::command()
                .try_get_matches_from(["tola", "nu", "-C", "site.toml", flag])
                .unwrap_err();
            assert_eq!(native.kind(), clap::error::ErrorKind::DisplayHelp);
            let matches = help_matches(&args(&["tola", "nu", "-C", "site.toml", flag])).unwrap();
            assert_eq!(site_help(&matches).unwrap().page, Page::Nu);

            for entry in ["script.nu", "@tola-scripts/image", "@tola-modules/catalog"] {
                let argv = ["tola", "nu", "-C", "site.toml", entry, flag];
                assert!(Cli::command().try_get_matches_from(argv).is_ok());
                assert!(site_help(&help_matches(&args(&argv)).unwrap()).is_none());
            }
        }
    }
}
