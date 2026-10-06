//! Editor selection, configuration files, and shared merge policy.

mod emacs;
mod helix;
mod neovim;
mod sublime;
mod vscode;
mod zed;

use std::path::Path;

use anyhow::{Context, Result};

use super::site::EditorDirectory;
use crate::writes::FileWrites;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Editor {
    Vscode,
    Helix,
    Neovim,
    Emacs,
    Zed,
    Sublime,
}

impl Editor {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Vscode => "VS Code",
            Self::Helix => "Helix",
            Self::Neovim => "Neovim",
            Self::Emacs => "Emacs",
            Self::Zed => "Zed",
            Self::Sublime => "Sublime Text",
        }
    }

    /// Site-relative settings file Tola merges, when the editor has one.
    ///
    /// `None` means the editor reads its language-server registration somewhere Tola does not
    /// merge; [`Self::settings_destination`] says where the user keeps the printed settings.
    pub(super) fn settings_file(self) -> Option<&'static Path> {
        match self {
            Self::Vscode => Some(Path::new(".vscode/settings.json")),
            Self::Helix => Some(Path::new(".helix/languages.toml")),
            Self::Neovim | Self::Emacs | Self::Zed | Self::Sublime => None,
        }
    }

    /// Where a client's settings go, as a phrase following "Add this to ".
    pub(super) const fn settings_destination(self) -> &'static str {
        match self {
            Self::Vscode => "`.vscode/settings.json` in this site",
            Self::Helix => "`.helix/languages.toml` in this site",
            Self::Neovim => "your Neovim 0.11+ configuration",
            Self::Emacs => "your Emacs init file",
            Self::Zed => "`.zed/settings.json` in this site",
            Self::Sublime => "your `.sublime-project` file",
        }
    }

    /// What a client needs to know beside where its settings go, when it needs anything.
    pub(super) const fn settings_note(self) -> Option<&'static str> {
        match self {
            Self::Zed => Some(
                "Zed starts a language server only where an extension declares one, so install the \
                 Typst extension; this replaces the server that extension publishes with Tola.",
            ),
            Self::Emacs => Some(
                "Emacs derives the language id `typst` from `typst-ts-mode`, which the \
                 `typst-ts-mode` package opens for `.typ` files; lsp-mode maps it the same way.",
            ),
            Self::Sublime => Some(
                "Install the Typst syntax package: its base scope `text.typst` is the selector, \
                 and it is the language id `typst` LSP sends.",
            ),
            Self::Vscode | Self::Helix | Self::Neovim => None,
        }
    }

    /// `source` is the current text of [`Self::settings_file`]; `None` renders the
    /// complete settings a fresh site needs.
    fn required_settings(
        self,
        source: Option<&str>,
        directory: &EditorDirectory,
    ) -> Result<String> {
        match self {
            Self::Vscode => vscode::merge(source, directory),
            Self::Helix => helix::merge(source, directory),
            Self::Neovim => Ok(neovim::snippet(directory)),
            Self::Emacs => Ok(emacs::snippet(directory)),
            Self::Zed => Ok(zed::snippet(directory)),
            Self::Sublime => Ok(sublime::snippet(directory)),
        }
    }
}

pub(crate) fn settings(
    editor: Editor,
    root: &Path,
    packages: &tola_typst::PackageLocations,
) -> Result<String> {
    editor.required_settings(None, &EditorDirectory::initial(root, packages)?)
}

/// The selected editors whose settings Tola prints instead of writing, each named once.
pub(crate) fn manual_editors(editors: &[Editor]) -> Vec<Editor> {
    unique(editors)
        .filter(|editor| editor.settings_file().is_none())
        .collect()
}

/// The `<EDITOR>` names `editor setup` accepts, in the order it offers them.
pub(crate) fn names() -> String {
    <Editor as clap::ValueEnum>::value_variants()
        .iter()
        .filter_map(<Editor as clap::ValueEnum>::to_possible_value)
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn setup(
    writes: &mut FileWrites,
    directory: &EditorDirectory,
    editors: &[Editor],
) -> Result<Vec<(Editor, String)>> {
    let mut manual_settings = Vec::new();
    for editor in unique(editors) {
        match prepare_settings(writes, directory, editor)? {
            PreparedSettings::File { relative, settings } => {
                if !settings.is_current() {
                    writes.replace_file_if_unchanged(relative, settings.read, settings.publish)?;
                }
            }
            PreparedSettings::Manual(configuration) => {
                manual_settings.push((editor, configuration))
            }
        }
    }
    Ok(manual_settings)
}

pub(super) fn add_initial_files(
    writes: &mut FileWrites,
    directory: &EditorDirectory,
    editors: &[Editor],
) -> Result<()> {
    for editor in unique(editors) {
        if let Some(relative) = editor.settings_file() {
            writes.create_file(relative, editor.required_settings(None, directory)?)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub(super) enum ConfigurationIssue {
    #[error("editor configuration is not a regular file")]
    Read,
    #[error("editor configuration is not valid UTF-8")]
    Encoding,
    #[error("the settings Tola requires conflict with this file")]
    Merge,
}

pub(super) enum PreparedSettings {
    /// Merge the required settings into `relative`, replacing it only when it changed.
    File {
        relative: &'static Path,
        settings: MergedSettings,
    },
    /// Instructions the user pastes beside an editor-managed configuration.
    Manual(String),
}

/// One editor settings file as read from disk and as it should be published.
pub(super) struct MergedSettings {
    pub(super) read: Option<Vec<u8>>,
    pub(super) publish: String,
}

impl MergedSettings {
    pub(super) fn is_current(&self) -> bool {
        self.read.as_deref() == Some(self.publish.as_bytes())
    }
}

pub(super) fn prepare_settings(
    writes: &FileWrites,
    directory: &EditorDirectory,
    editor: Editor,
) -> Result<PreparedSettings> {
    let Some(relative) = editor.settings_file() else {
        return Ok(PreparedSettings::Manual(
            editor.required_settings(None, directory)?,
        ));
    };
    let relative_path = crate::terminal::display_path_as_given(relative);
    let read = writes
        .read_file(relative)
        .context(ConfigurationIssue::Read)
        .with_context(|| {
            format!(
                "Tola could not read `{relative_path}`; keep it a regular file, then rerun `tola editor setup`"
            )
        })?;
    let source = read
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .context(ConfigurationIssue::Encoding)
        .with_context(|| {
            format!(
                "`{relative_path}` is not valid UTF-8; save it as UTF-8, then rerun `tola editor setup`"
            )
        })?;
    let publish = editor
        .required_settings(source, directory)
        .context(ConfigurationIssue::Merge)
        .with_context(|| format!("Tola could not update `{relative_path}`"))?;
    Ok(PreparedSettings::File {
        relative,
        settings: MergedSettings { read, publish },
    })
}

fn setting_conflict(editor: &str, key: &str, current: String, required: String) -> anyhow::Error {
    let diff = format!(
        "--- current\n+++ required\n- {key} = {}\n+ {key} = {}\n\nNo files were changed. Set this key to the required value, then rerun `tola editor setup` with the same editors.",
        current.trim(),
        required.trim(),
    );
    anyhow::Error::msg(diff).context(format!(
        "{editor} setting `{key}` conflicts with Tola's required value"
    ))
}

fn unique(editors: &[Editor]) -> impl Iterator<Item = Editor> + '_ {
    editors
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, editor)| (!editors[..index].contains(&editor)).then_some(editor))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory at `root` with no site configuration selected.
    pub(super) fn editor_directory(root: &str) -> EditorDirectory {
        EditorDirectory::initial(Path::new(root), &Default::default()).unwrap()
    }

    /// The site configuration `path` selects, with no package roots or overrides.
    pub(super) fn loaded_config(path: Option<&Path>) -> crate::config::LoadedConfig {
        crate::config::load(
            path,
            tola_build::InputScope::Online,
            tola_typst::PackageLocations::default(),
            &crate::config::ConfigOverrides::default(),
        )
        .unwrap()
    }

    /// The snippet's settings, read back as the client reads them.
    pub(super) fn parsed_settings(snippet: &str) -> serde_json::Value {
        let json = snippet
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::from_str(&json).expect("the snippet is JSON a client can read")
    }

    /// The command vector a Lua snippet configures, decoded from its `vim.json.decode` long bracket.
    fn lua_command(snippet: &str) -> Vec<String> {
        let (_, configured) = snippet
            .split_once("cmd = ")
            .expect("the snippet declares its language server `cmd`");
        let (_, encoded) = configured
            .split_once("vim.json.decode(")
            .expect("the command is configured through `vim.json.decode`");
        let bracket = encoded.strip_prefix('[').expect("the long bracket opens");
        let depth = bracket
            .chars()
            .take_while(|character| *character == '=')
            .count();
        let content = bracket[depth..]
            .strip_prefix('[')
            .expect("the long bracket opens");
        let close = format!("]{}]", "=".repeat(depth));
        let json = content
            .split_once(&close)
            .expect("the Lua long bracket closes")
            .0;
        serde_json::from_str(json).expect("the configured command is JSON")
    }

    #[test]
    fn editor_commands_use_selected_config() {
        use clap::Parser;

        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("custom config.toml");
        std::fs::write(&selected, "[site]\ntitle = 'Selected'\n").unwrap();
        let loaded = loaded_config(Some(&selected));
        let directory = EditorDirectory::from_config(loaded.config()).unwrap();
        let helix: toml::Value = toml::from_str(&helix::merge(None, &directory).unwrap()).unwrap();
        let helix = &helix["language-server"]["tola-lsp"];
        let helix_command = std::iter::once(helix["command"].as_str().unwrap().to_owned())
            .chain(
                helix["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|argument| argument.as_str().unwrap().to_owned()),
            )
            .collect::<Vec<_>>();
        let neovim_command = lua_command(&neovim::snippet(&directory));

        for has_default in [false, true] {
            if has_default {
                std::fs::write(
                    root.path().join("tola.toml"),
                    "[site]\ntitle = 'Not selected'\n",
                )
                .unwrap();
            }
            for command in [&helix_command, &neovim_command] {
                let cli = crate::cli::Cli::try_parse_from(command).unwrap();
                let crate::cli::Commands::Lsp { config, .. } = cli.command else {
                    panic!("editor command must start the language server");
                };
                let loaded = loaded_config(config.path.as_deref());
                assert_eq!(loaded.config().site().title, "Selected");
            }
        }
    }

    #[test]
    fn conflict_leaves_editor_files_untouched() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("custom.toml");
        std::fs::write(&selected, "").unwrap();
        let settings_path = root.path().join(".vscode/settings.json");
        std::fs::create_dir(settings_path.parent().unwrap()).unwrap();
        let original = "{\n \"tola.configPath\":\"other.toml\" // keep\n}\n";
        std::fs::write(&settings_path, original).unwrap();
        let loaded = loaded_config(Some(&selected));

        let directory = crate::editor::EditorDirectory::from_config(loaded.config()).unwrap();
        let package_inputs =
            crate::editor::prepare_package_inputs(loaded.config(), &Default::default()).unwrap();
        assert!(
            crate::editor::setup(&directory, &[Editor::Helix, Editor::Vscode], package_inputs)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(settings_path).unwrap(), original);
        assert!(!root.path().join(".helix").exists());
        assert!(!root.path().join(".tola").exists());
    }

    /// A settings path reaches the language server as one argument, quoted by each client's format.
    #[test]
    #[cfg(unix)]
    fn quoted_and_backslashed_paths_round_trip() {
        let directory = editor_directory("/site \"quoted\"\\dir");
        for (snippet, pointer) in [
            (
                zed::snippet as fn(&EditorDirectory) -> String,
                "/lsp/tinymist/binary/arguments/2",
            ),
            (
                sublime::snippet as fn(&EditorDirectory) -> String,
                "/settings/LSP/tola-lsp/command/3",
            ),
        ] {
            assert_eq!(
                parsed_settings(&snippet(&directory))
                    .pointer(pointer)
                    .unwrap(),
                "/site \"quoted\"\\dir/tola.toml"
            );
        }
    }
}
