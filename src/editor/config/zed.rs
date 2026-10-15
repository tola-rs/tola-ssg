//! Zed language-server settings for a site's `.zed/settings.json`.

use super::EditorDirectory;

/// The published Typst extension registers `tinymist`; its binary override starts Tola.
pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let packages = super::package_source_directory();
    let settings = serde_json::json!({
        "lsp": {
            "tinymist": {
                "binary": {
                    "path": command.command,
                    "arguments": command.arguments,
                },
                "initialization_options": {
                    "packageSourceDirectory": packages,
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
    fn snippet_configures_package_navigation() {
        let settings = parsed_settings(&snippet(&editor_directory(".")));
        let settings = &settings["lsp"]["tinymist"];
        assert_eq!(settings["binary"]["path"], "tola");
        assert_eq!(settings["binary"]["arguments"], serde_json::json!(["lsp"]));
        assert_eq!(
            settings["initialization_options"]["packageSourceDirectory"],
            super::super::package_source_directory()
        );
    }
}
