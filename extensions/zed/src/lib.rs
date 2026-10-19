//! The Zed adapter: Zed starts the `tola lsp` the author installed, and this extension supplies only
//! the command and the two configuration hooks.

use zed_extension_api::{self as zed, settings::LspSettings, Command, LanguageServerId, Result, Worktree};

struct TolaExtension;

impl zed::Extension for TolaExtension {
    fn new() -> Self {
        Self
    }

    /// The host joins a relative command to the extension's own directory, so the path must be
    /// absolute or absent.
    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let Some(command) = worktree.which("tola") else {
            return Err("no `tola` command on PATH; install Tola or set `lsp.tola.binary.path`".into());
        };
        Ok(Command {
            command,
            args: vec!["lsp".into()],
            env: Default::default(),
        })
    }

    /// Zed merges the author's own `initialization_options` over this answer, and arrays replace
    /// whole, so a hook value is only the default the author did not write.
    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree("tola", worktree)?.initialization_options)
    }

    /// The server asks for its configuration under the section `tola`, so the answer is the
    /// settings object the author wrote below `lsp.tola.settings`.
    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(LspSettings::for_worktree("tola", worktree)?.settings)
    }
}

zed::register_extension!(TolaExtension);
