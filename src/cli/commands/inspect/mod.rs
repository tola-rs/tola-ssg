use crate::cancellation::Cancellation;
mod format;
mod view;

use std::io::{self, BufRead};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::output::CommandOutput;
use crate::cli::{
    ConfigFileArgs, IconInspectArgs, InspectCommand, SourceInspectArgs, TypstPackageArgs,
};
use crate::terminal::session::{self, Shown};
use serde_json::Value;
use tola_build::InputScope;

const SOURCE_LIST_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

pub(in crate::cli) fn run(
    config_file: ConfigFileArgs,
    packages: TypstPackageArgs,
    scope: InputScope,
    command: InspectCommand,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let loaded = crate::cli::config::load_site(
        &config_file,
        &packages,
        scope,
        &candidate_overrides(&command),
        output,
        cancellation,
    )?;
    let config = std::sync::Arc::new(loaded.into_config());
    match command {
        InspectCommand::Sources(args) => sources(args, &config, &resources, output, cancellation),
        InspectCommand::Icons(args) => icons(args, &config, &resources, output, cancellation),
        command => site(command, config, resources, output, cancellation),
    }
}

/// Invocation overrides the command applies when it builds a candidate.
///
/// Source and icon inspection build no candidate, so they load with no overrides.
fn candidate_overrides(command: &InspectCommand) -> crate::config::ConfigOverrides {
    match command {
        InspectCommand::Documents(args)
        | InspectCommand::Routes(args)
        | InspectCommand::Outputs(args)
        | InspectCommand::References(args) => crate::config::ConfigOverrides {
            build: crate::cli::config::build_overrides(&args.build),
            ..crate::config::ConfigOverrides::default()
        },
        InspectCommand::Sources(_) | InspectCommand::Icons(_) => {
            crate::config::ConfigOverrides::default()
        }
    }
}

/// Whether one inspection asks for the interactive table.
fn interactive(command: &InspectCommand) -> bool {
    match command {
        InspectCommand::Sources(args) => args.interactive,
        InspectCommand::Icons(args) => args.interactive,
        InspectCommand::Documents(args)
        | InspectCommand::Routes(args)
        | InspectCommand::Outputs(args)
        | InspectCommand::References(args) => args.interactive,
    }
}

/// The name one site projection is browsed under.
fn projection_name(command: &InspectCommand) -> &'static str {
    match command {
        InspectCommand::Documents(_) => "documents",
        InspectCommand::Routes(_) => "routes",
        InspectCommand::Outputs(_) => "outputs",
        InspectCommand::References(_) => "references",
        InspectCommand::Sources(_) => "sources",
        InspectCommand::Icons(_) => "icons",
    }
}

/// The rows of one projection value, in projection order.
fn rows(value: Value) -> Vec<Value> {
    match value {
        Value::Array(rows) => rows,
        other => vec![other],
    }
}

/// Browses one projection's rows; `false` when the terminal cannot draw the table.
fn browse(
    rows: Vec<Value>,
    projection: &str,
    raw: bool,
    target: Option<&std::path::Path>,
    output: &CommandOutput,
    cancellation: &Cancellation,
    encode: &dyn Fn(&[Value]) -> Result<String>,
) -> Result<bool> {
    let save = |path: &std::path::Path, encoded: &str, rows: usize| -> Result<()> {
        let mut bytes = encoded.as_bytes().to_vec();
        bytes.push(b'\n');
        tola_build::filesystem::atomic_write(path, &bytes).with_context(|| {
            format!(
                "Tola could not write the inspection rows to `{}`",
                path.display()
            )
        })?;
        // The log records that the export happened, never the encoded rows themselves.
        if let Some(log) = output.log() {
            log.record(
                "INFO",
                "inspect",
                serde_json::json!({ "target": path.display().to_string(), "rows": rows }),
            );
        }
        Ok(())
    };
    let token = cancellation.token();
    let cancelled = || token.is_cancelled();
    let sink = output.terminal().sink();
    let mut view = view::View::new(rows, encode, &save);
    view.set_projection(projection);
    view.set_raw(raw);
    if let Some(target) = target {
        view.set_target(target);
    }
    match session::show(&sink, output.terminal().palette(), &cancelled, &mut view)? {
        Shown::Plain => Ok(false),
        Shown::Interactive => {
            // The terminal is restored before the rows reach stdout.
            if let Some(encoded) = view.stdout_export() {
                output.write_stdout_line(encoded)?;
            }
            Ok(true)
        }
    }
}

/// Encodes rows the way the site projections print them.
fn pretty(rows: &[Value]) -> Result<String> {
    Ok(serde_json::to_string_pretty(&Value::Array(rows.to_vec()))?)
}

/// List configured icon namespaces, or one namespace's icon names.
fn icons(
    args: IconInspectArgs,
    config: &tola_build::config::ResolvedSiteConfig,
    resources: &tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    if let Some(namespace) = args.namespace.as_deref()
        && !config.icons().collections.contains_key(namespace)
    {
        return Err(unknown_icon_namespace(namespace, config.icons()));
    }
    output.status("Preparing icon collections…")?;
    let value = tola_build::inspect::icons(
        config,
        resources,
        &cancellation.token(),
        args.namespace.as_deref(),
    )?;
    let rows = rows(value);
    let encoded = pretty(&rows)?;
    if args.interactive && browse(rows, "icons", false, None, output, cancellation, &pretty)? {
        return Ok(());
    }
    output.write_stdout_line(encoded)?;
    Ok(())
}

/// A selection that names no configured namespace, bounded so one message stays readable.
fn unknown_icon_namespace(
    namespace: &str,
    icons: &tola_build::config::IconsConfig,
) -> anyhow::Error {
    if icons.collections.is_empty() {
        return anyhow::anyhow!(
            "the site configures no icon collections, so `{namespace}` has no icons"
        );
    }
    let mut configured = icons.collections.keys().map(String::as_str);
    let named = configured
        .by_ref()
        .take(3)
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let listed = match configured.count() {
        0 => named,
        remaining => format!("{named}, and {remaining} more"),
    };
    anyhow::anyhow!("no icon collection is named `{namespace}`; the site configures {listed}")
}

fn sources(
    args: SourceInspectArgs,
    config: &tola_build::config::ResolvedSiteConfig,
    resources: &tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    let paths = source_paths(args.paths, cancellation)?;
    output.secondary("Inspecting sources…")?;
    let sources =
        tola_build::inspect::inspect_sources(&paths, config, resources, &cancellation.token())?;
    cancellation.token().ensure_active()?;
    output.status(format!(
        "Inspected {}; {} with metadata",
        crate::terminal::plural_count(sources.selected(), "source"),
        sources.matched()
    ))?;
    output.diagnostics(sources.diagnostics())?;
    let options = format::SourceFormat {
        raw: args.raw,
        pretty: args.pretty,
        filter_empty: args.filter_empty,
        fields: args.fields,
    };
    if args.interactive {
        let rows = format::source_rows(&sources, &options)?;
        let encode = |rows: &[Value]| format::encode(rows, &options);
        if browse(
            rows,
            "sources",
            args.raw,
            args.output.as_deref(),
            output,
            cancellation,
            &encode,
        )? {
            return Ok(());
        }
    }
    let encoded = format::sources(&sources, &options)?;
    if let Some(path) = args.output {
        let mut bytes = encoded.into_bytes();
        bytes.push(b'\n');
        tola_build::filesystem::atomic_write(&path, &bytes).with_context(|| {
            format!(
                "Tola could not write the source metadata to `{}`",
                path.display()
            )
        })?;
    } else {
        output.write_stdout_line(encoded)?;
    }
    Ok(())
}

fn site(
    command: InspectCommand,
    config: std::sync::Arc<tola_build::config::ResolvedSiteConfig>,
    resources: tola_build::BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let skipped = enabled_hook_count(&config);
    output.status("Building site for inspection…")?;
    if skipped > 0 {
        output.status(format!(
            "Skipping {}; run `tola check` to run hooks",
            crate::terminal::plural_count(skipped, "enabled build hook command")
        ))?;
    }
    let token = cancellation.token();
    let locked = super::build_candidate(
        config,
        cancellation,
        tola_build::build::HookExecution::Skip,
        resources,
        output,
    )?;
    locked.ensure_fresh(&token)?;
    let site = locked.release();
    cancellation.token().ensure_active()?;
    output.diagnostics(site.diagnostics())?;
    let interactive = interactive(&command);
    let projection = projection_name(&command);
    let value = match command {
        InspectCommand::Documents(_) => tola_build::inspect::documents(&site),
        InspectCommand::Routes(_) => tola_build::inspect::routes(&site),
        InspectCommand::Outputs(_) => tola_build::inspect::outputs(&site),
        InspectCommand::References(_) => tola_build::inspect::references(&site),
        InspectCommand::Sources(_) => unreachable!("sources use the source inspection path"),
        InspectCommand::Icons(_) => unreachable!("icons use the icon inspection path"),
    };
    let rows = rows(value);
    let encoded = pretty(&rows)?;
    if interactive && browse(rows, projection, false, None, output, cancellation, &pretty)? {
        return Ok(());
    }
    output.write_stdout_line(encoded)?;
    Ok(())
}

/// Enabled hook commands that a complete production candidate would run.
///
/// Every enabled command counts, except the `after-publish` consumers, which consume a committed
/// revision that inspection never produces.
fn enabled_hook_count(config: &tola_build::config::ResolvedSiteConfig) -> usize {
    let hooks = &config.build().hooks;
    let after_publish = hooks
        .after_publish
        .iter()
        .filter(|hook| hook.enable)
        .count();
    hooks.enabled_commands().count() - after_publish
}

/// Read source paths from stdin when the sole argument is `-`.
fn source_paths(paths: Vec<PathBuf>, cancellation: &Cancellation) -> Result<Vec<PathBuf>> {
    if paths.len() != 1 || paths[0].as_os_str() != "-" {
        return Ok(paths);
    }

    let (lines, incoming) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("source-list".to_owned())
        .spawn(move || {
            for line in io::stdin().lock().lines() {
                let failed = line.is_err();
                if lines.send(line).is_err() || failed {
                    break;
                }
            }
        })?;
    collect_source_paths(incoming, &cancellation.token())
}

fn collect_source_paths(
    incoming: std::sync::mpsc::Receiver<io::Result<String>>,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    loop {
        cancellation.ensure_active()?;
        match incoming.recv_timeout(SOURCE_LIST_POLL_INTERVAL) {
            Ok(line) => {
                let line = line.context("failed to read a source path from stdin")?;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    paths.push(PathBuf::from(trimmed));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(paths),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_list_skips_empty_lines() {
        let (lines, incoming) = std::sync::mpsc::channel();
        for line in [" first.typ ", "", "  ", "nested/second.typ"] {
            lines.send(Ok(line.to_owned())).unwrap();
        }
        drop(lines);
        let paths = collect_source_paths(
            incoming,
            &tola_build::cancellation::BuildCancellation::default(),
        )
        .unwrap();
        assert_eq!(
            paths,
            [
                PathBuf::from("first.typ"),
                PathBuf::from("nested/second.typ")
            ]
        );
    }

    #[test]
    fn source_list_cancellation_returns_early() {
        let (_lines, incoming) = std::sync::mpsc::channel();
        let canceller = tola_build::cancellation::BuildCanceller::default();
        canceller.cancel();
        let error = collect_source_paths(incoming, &canceller.token()).unwrap_err();
        assert!(error.is::<tola_build::cancellation::BuildCancelled>());
    }
}
