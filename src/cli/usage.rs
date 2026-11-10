//! Best-effort discovery and rendering for static CLI help.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use clap::{ColorChoice, CommandFactory};
use tola_input::InputScope;
use tola_module::manifest::ModuleDeclarations;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Page {
    Top,
    Queries,
    Nu,
}

/// The help page this invocation asks for, when it asks for one.
///
/// Only the leading tokens are read: the first non-option word names the command whose help the
/// renderer shows. Every other command's help stays with clap.
fn page_request(args: &[OsString]) -> Option<(Page, bool)> {
    if args.len() == 1 {
        return Some((Page::Top, false));
    }
    let mut long = false;
    let mut short = false;
    let mut first = None;
    let mut index = 1;
    while index < args.len() {
        let argument = args[index].to_str()?;
        if argument == "--" {
            break;
        }
        match argument {
            "--help" => long = true,
            "-h" => short = true,
            "--color" | "--log-file" => index += 1,
            value if value.starts_with('-') => {}
            value if first.is_none() => first = Some(value),
            _ => {}
        }
        index += 1;
    }
    if !(long || short) {
        return None;
    }
    let page = match first {
        None => Page::Top,
        Some("query") => Page::Queries,
        Some("nu") => Page::Nu,
        Some(_) => return None,
    };
    Some((page, long))
}

fn discover(page: Page, scope: InputScope) -> anyhow::Result<BTreeMap<String, ModuleDeclarations>> {
    let source = tola_build::config::ConfigSource::load(None, scope)?;
    let (core, _) = source.decode_with_host::<crate::config::ConfigSchema>()?;
    let schema = core.into_schema();
    let mut modules = tola_build::modules::manifest::read_declarations(
        source.root(),
        None,
        &schema.modules,
        tola_input::InputMode::new(scope, true),
        &Default::default(),
    )?;
    let builtins = super::Cli::command();
    anyhow::ensure!(
        builtins
            .get_subcommands()
            .all(|c| !modules.contains_key(c.get_name())),
        "reserved module namespace"
    );
    if page == Page::Top
        && let Ok(commands) = tola_build::modules::select_command_modules(
            source.root(),
            None,
            &schema.modules,
            tola_input::InputMode::new(scope, true),
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
            .map(|(name, _)| (format!("@tola-modules/{name}"), ""))
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
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let (page, long) = page_request(&arguments)?;
    let mut help = render_page(page, long, color_choice(&arguments))?;
    if page == Page::Nu {
        append_builtin_nu(&mut help);
    }
    if let Ok(modules) = discover(page, help_scope(page)) {
        append_site_help(&mut help, &page, &modules);
    }
    Some(emit(help))
}

fn color_choice(arguments: &[OsString]) -> ColorChoice {
    if crate::terminal::color::enabled(
        super::argv::color_hint(arguments),
        crate::terminal::color::Stream::Stdout,
    ) {
        ColorChoice::Always
    } else {
        ColorChoice::Never
    }
}

fn render_page(page: Page, long: bool, color: ColorChoice) -> Option<String> {
    let command = super::Cli::command().color(color);
    // `bin_name` takes the subcommand by value, so the documented way to change it is
    // `mut_subcommand`; cloning the subcommand back into place is the workaround it replaces.
    let mut command = match page {
        Page::Top => command,
        Page::Queries => {
            command.mut_subcommand("query", |query| query.bin_name("tola query").color(color))
        }
        Page::Nu => command.mut_subcommand("nu", |nu| nu.bin_name("tola nu").color(color)),
    };
    // Global arguments are cloned into each subcommand during the parent's build; a subcommand
    // rendered on its own has none until this runs.
    command.build();
    let target = match page {
        Page::Top => &mut command,
        Page::Queries => command.find_subcommand_mut("query")?,
        Page::Nu => command.find_subcommand_mut("nu")?,
    };
    let rendered = if long {
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
    Some(help)
}

fn append_builtin_nu(help: &mut String) {
    help.push_str(
        "\nRoot query: `use tola` then `tola query`, or `use tola query` then `query`.\n",
    );
    help.push_str("\nBuilt-in Nu modules (import with `use tola NAME`):\n");
    for (name, summary) in tola_nu::BUILTIN_MODULES {
        help.push_str(&format!("  @tola-scripts/{name:<16} {summary}\n"));
    }
    help.push_str(
        "\nRun a module with `tola nu @tola-scripts/NAME --help` when it exports main.\n",
    );
}

fn help_scope(page: Page) -> InputScope {
    if page != Page::Nu {
        return InputScope::Online;
    }
    let inherited = std::env::var("TOLA_INPUT_SCOPE").ok();
    InputScope::parse(inherited.as_deref().unwrap_or("online")).unwrap_or(InputScope::Online)
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
    fn page_request_reads_the_leading_command() {
        assert_eq!(page_request(&args(&["tola"])), Some((Page::Top, false)));
        assert_eq!(
            page_request(&args(&["tola", "query", "--help"])),
            Some((Page::Queries, true))
        );
        assert_eq!(page_request(&args(&["tola", "build", "--help"])), None);
    }
}
