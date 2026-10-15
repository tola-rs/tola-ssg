//! Sublime Text LSP client settings for a site's `.sublime-project`.

use super::EditorDirectory;

/// The LSP package reads a project's `settings.LSP` object, keyed by client name, and starts a
/// client whose `selector` matches the buffer's base scope. `text.typst` is the base scope of the
/// Typst syntax package, which the package turns into the language id `typst`, and the
/// configuration document of the site is not a Typst buffer, so one client answers Typst alone.
pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let executable = directory.server_command().argv();
    let packages = super::package_source_directory();
    let settings = serde_json::json!({
        "settings": {
            "LSP": {
                "tola-lsp": {
                    "enabled": true,
                    "command": executable,
                    "selector": "text.typst",
                    "initialization_options": {
                        "packageSourceDirectory": packages,
                    },
                },
            },
        },
    });
    super::settings_text(&settings)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{editor_directory, parsed_settings};
    use super::*;

    #[test]
    fn client_configures_package_navigation() {
        let settings = parsed_settings(&snippet(&editor_directory(".")));
        let client = &settings["settings"]["LSP"]["tola-lsp"];
        assert_eq!(client["selector"], "text.typst");
        assert_eq!(client["command"], serde_json::json!(["tola", "lsp"]));
        assert_eq!(
            client["initialization_options"]["packageSourceDirectory"],
            super::super::package_source_directory()
        );
    }
}
