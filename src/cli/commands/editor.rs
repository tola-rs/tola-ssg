//! `tola editor`: site-local editor setup, the clients detected here, and explicit package inputs.
//!
//! Setup and its dry run share one checked preparation of the merged files, package links, and
//! obsolete-path removals; only a run without `--dry-run` writes any of them.
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::cancellation::Cancellation;
use crate::cli::log::LogFile;
use crate::cli::log::destination;
use crate::cli::output::CommandOutput;
use crate::cli::{ConfigFileArgs, EditorCommand, TypstPackageArgs};
use crate::config::LoadedConfig;
use crate::editor::{Editor, EditorDirectory, EditorPackageInputs};
use tola_build::InputScope;

pub(in crate::cli) fn run(
    command: EditorCommand,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    match command {
        EditorCommand::Template { editor } => {
            destination::start(output, &cancellation.token())?;
            let packages = crate::cli::config::package_locations(&Default::default(), scope)?;
            output.status(crate::editor::settings_instructions(editor))?;
            output.write_stdout(crate::editor::settings(editor, Path::new("."), &packages)?)?;
            Ok(())
        }
        EditorCommand::Setup {
            config,
            packages,
            editors,
            list,
            dry_run,
        } => {
            let (loaded, directory) =
                load_editor_context(&config, &packages, scope, output, cancellation)?;
            if list {
                start_session_log(output, loaded.as_ref(), cancellation)?;
                output.block(crate::editor::client_list(&directory))?;
                return Ok(());
            }
            let editors = if editors.is_empty() {
                select_editors(scope, cancellation)?
            } else {
                editors
            };
            let package_inputs = editor_package_inputs(
                loaded.as_ref(),
                &directory,
                &packages,
                scope,
                &cancellation.token(),
            )?;
            let setup = crate::editor::setup(&directory, &editors, package_inputs)?;
            for file in setup.writes.file_paths() {
                destination::check_file(output.log().map(LogFile::path), file)?;
            }
            start_session_log(output, loaded.as_ref(), cancellation)?;
            cancellation.token().ensure_active()?;
            if dry_run {
                setup.writes.check()?;
                let obsolete = crate::editor::obsolete_package_paths(directory.root())?;
                output.block(crate::editor::dry_run_report(&setup, &obsolete))?;
                return Ok(());
            }
            let settings_report = crate::editor::settings_report(&setup, &editors);
            setup.writes.apply(&cancellation.token())?;
            setup.package_inputs.apply(&cancellation.token())?;
            crate::editor::remove_obsolete_package_paths(directory.root())?;
            if !settings_report.is_empty() {
                output.block(settings_report)?;
            }
            for (editor, settings) in setup.manual_settings {
                output.block(crate::editor::manual_settings_instructions(
                    editor, &settings,
                ))?;
            }
            output.summary("Editor setup complete")?;
            output.status(crate::editor::startup_instructions(&editors))?;
            Ok(())
        }
        EditorCommand::Packages { config, packages } => {
            let (loaded, directory) =
                load_editor_context(&config, &packages, scope, output, cancellation)?;
            let package_inputs = editor_package_inputs(
                loaded.as_ref(),
                &directory,
                &packages,
                scope,
                &cancellation.token(),
            )?;
            let refresh = crate::editor::refresh_packages(&directory, package_inputs)?;
            for file in refresh.writes.file_paths() {
                destination::check_file(output.log().map(LogFile::path), file)?;
            }
            start_session_log(output, loaded.as_ref(), cancellation)?;
            cancellation.token().ensure_active()?;
            refresh.writes.apply(&cancellation.token())?;
            refresh.package_inputs.apply(&cancellation.token())?;
            crate::editor::remove_obsolete_package_paths(directory.root())?;
            output.summary("Updated editor packages")?;
            Ok(())
        }
    }
}

/// The site context both editor package commands share: the loaded configuration when the
/// directory is a site, and the editor directory it configures.
fn load_editor_context(
    config: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<(Option<LoadedConfig>, EditorDirectory)> {
    let loaded = crate::cli::config::load_optional_config(
        config,
        packages,
        scope,
        &crate::config::ConfigOverrides::default(),
        output,
        cancellation,
    )?;
    if let Some(loaded) = &loaded {
        output.apply_diagnostic_limits(loaded.diagnostics());
    }
    let directory = editor_directory(config, packages, scope, loaded.as_ref())?;
    Ok((loaded, directory))
}

/// The package inputs to install: the site's resolved package locations, or the discovered
/// locations when no site is loaded.
fn editor_package_inputs(
    loaded: Option<&LoadedConfig>,
    directory: &EditorDirectory,
    packages: &TypstPackageArgs,
    scope: InputScope,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<EditorPackageInputs> {
    match loaded {
        Some(loaded) => crate::editor::prepare_package_inputs(
            loaded.config(),
            &crate::cli::config::build_resources(scope),
        ),
        None => crate::editor::initial_package_inputs(
            directory.root(),
            &crate::cli::config::package_locations(packages, scope)?,
            cancellation,
        ),
    }
}

/// The editor directory this invocation configures: the site it names or finds, or the working
/// directory when no `tola.toml` sits in it or above it.
fn editor_directory(
    config: &ConfigFileArgs,
    packages: &TypstPackageArgs,
    scope: InputScope,
    loaded: Option<&LoadedConfig>,
) -> Result<EditorDirectory> {
    match loaded {
        Some(loaded) => EditorDirectory::from_config(loaded.config()),
        None => {
            let root = crate::cli::config::source_root(config)?;
            let packages = crate::cli::config::package_locations(packages, scope)?;
            EditorDirectory::workspace(&root, &packages)
        }
    }
}

/// Start this session's log where the site's own directory allows it, or where the command runs.
fn start_session_log(
    output: &CommandOutput,
    loaded: Option<&LoadedConfig>,
    cancellation: &Cancellation,
) -> Result<()> {
    match loaded {
        Some(loaded) => destination::start_site(output, loaded.config(), &cancellation.token()),
        None => destination::start(output, &cancellation.token()),
    }
}

#[derive(Serialize)]
struct EditorChoice {
    index: usize,
    editor: String,
}

fn select_editors(scope: InputScope, cancellation: &Cancellation) -> Result<Vec<Editor>> {
    if !crate::cli::interaction::available() {
        anyhow::bail!(
            "no editor specified; run `tola editor setup <EDITOR>...` with {}",
            crate::editor::names(),
        );
    }
    let choices = crate::editor::selection_choices()
        .into_iter()
        .enumerate()
        .map(|(index, label)| EditorChoice {
            index,
            editor: label,
        })
        .collect::<Vec<_>>();
    let selected: Option<Vec<usize>> = crate::cli::interaction::run(
        include_str!("editor_select.nu"),
        &choices,
        scope,
        cancellation,
    )?;
    let selected = selected.ok_or(crate::terminal::InputCancelled)?;
    if selected.is_empty() {
        anyhow::bail!(
            "no editor selected; run `tola editor setup` again and choose at least one editor"
        );
    }
    let editors = <Editor as clap::ValueEnum>::value_variants();
    selected
        .into_iter()
        .map(|index| {
            editors
                .get(index)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("editor selection returned an invalid choice"))
        })
        .collect()
}
