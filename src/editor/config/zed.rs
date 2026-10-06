//! Zed language-server settings for a site's `.zed/settings.json`.

use super::EditorDirectory;

/// Zed starts a language server only where an extension declares one, so a client names the server
/// that extension published: for Typst that is the official Typst extension's `tinymist` entry,
/// whose configured binary and arguments Tola replaces with its own.
///
/// A settings `path` that names no file inside the site resolves through `PATH`, and an absolute
/// path stays the file it names, so the site keeps working when Tola is upgraded elsewhere.
pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let settings = serde_json::json!({
        "lsp": {
            "tinymist": {
                "binary": {
                    "path": command.command,
                    "arguments": command.arguments,
                },
            },
        },
    });
    let mut text = String::from(
        "// Zed starts a language server only where an extension declares one. Install the Typst\n\
         // extension, then keep this in the site's `.zed/settings.json`: it answers the extension's\n\
         // `Typst` language with Tola instead of the server the extension publishes.\n",
    );
    text.push_str(
        &serde_json::to_string_pretty(&settings).expect("editor settings are serializable"),
    );
    crate::writes::with_final_newline(text)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{editor_directory, parsed_settings};
    use super::*;

    #[test]
    fn snippet_answers_the_extension_language() {
        let settings = parsed_settings(&snippet(&editor_directory(".")));
        assert_eq!(settings["lsp"]["tinymist"]["binary"]["path"], "tola");
        assert_eq!(
            settings["lsp"]["tinymist"]["binary"]["arguments"],
            serde_json::json!(["lsp", "--config", "./tola.toml"])
        );
    }
}
