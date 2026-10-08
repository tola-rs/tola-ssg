//! Site-local editor configuration and explicit package input preparation.

mod config;
pub(crate) mod launch;
mod packages;
mod site;

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;

use crate::writes::FileWrites;
pub(crate) use site::EditorDirectory;

pub(crate) use config::{Editor, manual_editors, names, settings};

/// Whether this editor's settings are files Tola writes, rather than settings it prints.
pub(crate) fn writes_settings(editor: Editor) -> bool {
    editor.settings_file().is_some()
}
pub(crate) use packages::{
    EditorPackageInputs, GENERATED_PACKAGE_DIRECTORY, generated_package_files,
    initial_package_inputs, obsolete_package_paths, prepare_package_inputs,
    remove_obsolete_package_paths,
};

/// `init` and `editor setup` report the same policy, so the wording lives here once.
pub(crate) const VSCODE_EXTENSION_NOTICE: &str =
    "Install the Tola VS Code extension; it never changes settings it does not own";

pub(crate) const RESTART_CLIENTS_NOTICE: &str =
    "Restart the selected editors after saving their settings";

/// Where one editor's settings go and what its client needs to know about them.
pub(crate) fn settings_instructions(editor: Editor) -> String {
    let mut text = format!("Add this to {}:", editor.settings_destination());
    if let Some(note) = editor.settings_note() {
        text.push('\n');
        text.push_str(note);
    }
    text
}

/// The settings for one editor Tola does not write, with where they go and what the client needs
/// to know about them.
pub(crate) fn manual_settings_instructions(editor: Editor, settings: &str) -> String {
    format!(
        "{} — copy these settings manually\n{}\n\n{}",
        editor.name(),
        settings_instructions(editor),
        settings.trim_end()
    )
}

/// One line per supported editor: its name and what `editor setup` does with its settings.
pub(crate) fn selection_choices() -> Vec<String> {
    <Editor as clap::ValueEnum>::value_variants()
        .iter()
        .map(|editor| {
            let action = if writes_settings(*editor) {
                "update settings file"
            } else {
                "print settings to copy manually"
            };
            format!("{} ({action})", editor.name())
        })
        .collect()
}

/// One screen for a client Tola does not configure: every editor it can configure, and the
/// command, arguments, file extensions, and language ids such a client needs for this directory.
pub(crate) fn client_list(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let mut text = String::from("Editor setup options:\n");
    for editor in <Editor as clap::ValueEnum>::value_variants() {
        let _ = writeln!(
            text,
            "  {:<14}{}",
            editor.name(),
            match editor.settings_file() {
                Some(relative) => format!(
                    "writes {}",
                    crate::terminal::display_path_as_given(relative)
                ),
                None => format!(
                    "copy settings manually to {}",
                    editor.settings_destination()
                ),
            }
        );
    }
    text.push_str("\nAny other LSP client, over stdio:\n");
    let _ = writeln!(text, "  command: {}", command.command);
    let _ = writeln!(
        text,
        "  args:    {}",
        command
            .arguments
            .iter()
            .map(|argument| site::shell_word(argument))
            .collect::<Vec<_>>()
            .join(" ")
    );
    text.push_str("  files:   `.typ` sources, language id `typst`\n");
    if directory.names_configuration_file() {
        text.push_str(
            "           and the configuration file `--config` names, language id `toml`\n",
        );
    }

    text
}

pub(crate) struct EditorSetup {
    pub(crate) writes: FileWrites,
    pub(crate) manual_settings: Vec<(Editor, String)>,
    pub(crate) package_inputs: EditorPackageInputs,
}

pub(crate) fn settings_report(setup: &EditorSetup, editors: &[Editor]) -> String {
    let mut text = String::new();
    for (index, editor) in editors.iter().enumerate() {
        if editors[..index].contains(editor) {
            continue;
        }
        let Some(relative) = editor.settings_file() else {
            continue;
        };
        let action = if setup.writes.file_contents(relative).is_some() {
            "updated"
        } else {
            "already current"
        };
        let _ = writeln!(
            text,
            "{}: `{}` {action}",
            editor.name(),
            crate::terminal::display_path_as_given(relative),
        );
    }
    text
}

/// Staged editor package files and package view, with no editor settings.
pub(crate) struct EditorPackageRefresh {
    pub(crate) writes: FileWrites,
    pub(crate) package_inputs: EditorPackageInputs,
}

/// Stage the generated package files and the package view. Editor settings stay untouched, so
/// `editor packages` and `editor setup` share this step.
pub(crate) fn refresh_packages(
    directory: &EditorDirectory,
    package_inputs: EditorPackageInputs,
) -> Result<EditorPackageRefresh> {
    let root = directory.root();
    let mut writes = FileWrites::new(root)?;
    packages::ensure_generated_package_paths_are_safe(root)?;
    packages::add_editor_package_replacements(&mut writes)?;
    Ok(EditorPackageRefresh {
        writes,
        package_inputs,
    })
}

pub(crate) fn setup(
    directory: &EditorDirectory,
    editors: &[Editor],
    package_inputs: EditorPackageInputs,
) -> Result<EditorSetup> {
    let refresh = refresh_packages(directory, package_inputs)?;
    let mut writes = refresh.writes;
    let manual_settings = config::setup(&mut writes, directory, editors)?;

    Ok(EditorSetup {
        writes,
        manual_settings,
        package_inputs: refresh.package_inputs,
    })
}

/// Every line `editor setup --dry-run` prints: the staged editor settings files with their
/// contents, the generated package files and the package view, the package paths setup would
/// remove, and each editor's manual configuration.
pub(crate) fn dry_run_report(setup: &EditorSetup, obsolete: &[PathBuf]) -> String {
    let writes = &setup.writes;
    let root = writes.root();
    let mut settings_files = Vec::new();
    let mut generated_files = Vec::new();
    for path in writes.file_paths() {
        let relative = writes.relative_path(path);
        if relative.starts_with(GENERATED_PACKAGE_DIRECTORY) {
            generated_files.push(relative);
        } else {
            settings_files.push(relative);
        }
    }

    let mut text = format!(
        "Editor setup for {} (dry run; nothing applied)\n",
        crate::terminal::display_path(root)
    );

    text.push_str("\nEditor settings files:\n");
    if settings_files.is_empty() {
        text.push_str("  (none)\n");
    }
    for relative in settings_files {
        let _ = writeln!(
            text,
            "  {}",
            crate::terminal::display_path_as_given(relative)
        );
        let contents = writes
            .file_contents(relative)
            .expect("a staged file has staged contents");
        text.push_str(&String::from_utf8_lossy(contents));
    }

    text.push_str("\nGenerated package files:\n");
    if generated_files.is_empty() {
        text.push_str("  (none)\n");
    }
    for relative in generated_files {
        let _ = writeln!(
            text,
            "  {}",
            crate::terminal::display_path_as_given(relative)
        );
    }

    text.push_str("\nEditor package view:\n");
    for directory in setup.package_inputs.view_directories() {
        let _ = writeln!(
            text,
            "  directory {}",
            crate::terminal::display_path_within(&directory, root)
        );
    }
    for (link, target) in setup.package_inputs.view_links() {
        let _ = writeln!(
            text,
            "  link {} -> {}",
            crate::terminal::display_path_within(&link, root),
            crate::terminal::display_path_within(target, root)
        );
    }

    text.push_str("\nGenerated package paths to remove:\n");
    if obsolete.is_empty() {
        text.push_str("  (none)\n");
    }
    for path in obsolete {
        let _ = writeln!(
            text,
            "  {}",
            crate::terminal::display_path_within(path, root)
        );
    }

    text.push_str("\nManual configuration:\n");
    if setup.manual_settings.is_empty() {
        text.push_str("  (none)\n");
    }
    for (editor, settings) in &setup.manual_settings {
        let _ = writeln!(text, "{}", manual_settings_instructions(*editor, settings));
    }

    text
}

/// Collect the editor files for an init preview without inspecting their destinations.
pub(crate) fn add_initial_files(
    writes: &mut FileWrites,
    editors: &[Editor],
    packages: &tola_typst::PackageLocations,
) -> Result<()> {
    packages::add_initial_files(writes)?;
    let directory = EditorDirectory::initial(writes.root(), packages)?;
    config::add_initial_files(writes, &directory, editors)
}

pub(crate) fn check_initial_files(root: &std::path::Path) -> Result<()> {
    packages::ensure_generated_package_paths_are_safe(root)
}
