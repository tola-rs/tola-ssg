//! `tola inspect`: read-only views of the inputs, resources, and configuration Tola resolved.
//!
//! A view loads and prepares configuration but compiles no Bundle and writes no site output, so it
//! reports declarations, discovered files, and selected paths rather than anything rendered.
//! Results are written through `super::read`, which owns JSON and file output.
use crate::cancellation::Cancellation;

use std::io::{self, BufRead};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tola_input::{InputMode, InputScope};

use crate::cli::output::CommandOutput;
use crate::cli::{
    BuildArgs, ConfigFileArgs, IconInspectArgs, InspectCommand, SiteArgs, SourceInspectArgs,
};

const SOURCE_LIST_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

pub(in crate::cli) fn run(
    config: ConfigFileArgs,
    command: InspectCommand,
    mode: InputMode,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let scope = mode.scope();
    match command {
        InspectCommand::Fonts {
            packages,
            fonts,
            display,
            ..
        } => {
            let request = super::SiteRequest::new(
                SiteArgs { config, packages },
                BuildArgs::from_fonts(fonts),
                mode,
                output,
                cancellation,
            )?;
            let value = super::fonts::value(&request)?;
            super::read::write(&value, "fonts", &display, scope, output, cancellation)
        }
        InspectCommand::Config(args) => {
            let request = super::SiteRequest::new(
                SiteArgs {
                    config,
                    packages: args.packages,
                },
                BuildArgs::default(),
                mode,
                output,
                cancellation,
            )?;
            let value = super::config::value(&request, &args.build, args.mode)?;
            super::read::write(&value, "config", &args.display, scope, output, cancellation)
        }
        command => {
            let request = super::SiteRequest::new(
                SiteArgs {
                    config,
                    ..Default::default()
                },
                BuildArgs::default(),
                mode,
                output,
                cancellation,
            )?;
            let loaded = request.load_config(&crate::config::ConfigOverrides::default())?;
            let config = loaded.config();
            match command {
                InspectCommand::Sources(args) => sources(
                    args,
                    config,
                    &request.resources,
                    scope,
                    output,
                    cancellation,
                ),
                InspectCommand::Icons(args) => icons(
                    args,
                    config,
                    &request.resources,
                    scope,
                    output,
                    cancellation,
                ),
                _ => unreachable!("font and config inspection return above"),
            }
        }
    }
}

fn icons(
    args: IconInspectArgs,
    config: &tola_build::config::ResolvedSiteConfig,
    resources: &tola_build::BuildResources,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let prepared =
        tola_build::modules::prepare_site(config, resources.input_mode(), &cancellation.token())?;
    let config = prepared.config();
    let namespace = args
        .namespace
        .as_deref()
        .map(|name| {
            tola_icons::IconCollectionKey::new(args.module.as_deref(), name)
                .map(|key| key.to_string())
        })
        .transpose()?;
    if let Some(module) = &args.module {
        anyhow::ensure!(
            config.modules().contains_key(module),
            "module `{module}` is not configured"
        );
    }
    if let Some(namespace) = namespace.as_deref()
        && !config.icons().collections.contains_key(namespace)
    {
        return Err(unknown_icon_namespace(namespace, config.icons()));
    }
    output.status("Preparing icon collections…")?;
    let mut value = tola_build::inspect::icons(
        config,
        resources,
        &cancellation.token(),
        namespace.as_deref(),
    )?;
    if let Some(module) = args.module
        && let Some(rows) = value.as_array_mut()
    {
        rows.retain(|row| row["module"].as_str() == Some(module.as_str()));
    }
    super::read::write(&value, "icons", &args.display, scope, output, cancellation)
}

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
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let paths = source_paths(args.paths, cancellation)?;
    let sources =
        tola_build::inspect::content_sources(&paths, config, resources, &cancellation.token())?;
    let rows = sources
        .iter()
        .map(|source| {
            json!({
                "path": tola_diagnostic::display_path(&source.source, config.get_root()),
                "contentPath": source.id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    super::read::write(
        &Value::Array(rows),
        "sources",
        &args.display,
        scope,
        output,
        cancellation,
    )
}

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
                let line = line.context("Tola could not read a source path from stdin")?;
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
