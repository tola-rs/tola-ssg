//! Emacs language-server registration for Eglot and lsp-mode, as an init-file snippet.

use super::EditorDirectory;

/// Eglot's contact keeps initialization options after the command; lsp-mode takes them on the client.
pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let mut words = vec![elisp_string(command.command)];
    words.extend(
        command
            .arguments
            .iter()
            .map(|argument| elisp_string(argument)),
    );
    let arguments = words.join(" ");
    let packages = elisp_string(&super::package_source_directory());
    format!(
        r#";; Eglot (Emacs 29+), in the author's init file.
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '(typst-ts-mode . ({arguments}
                 :initializationOptions (:packageSourceDirectory {packages})))))

;; lsp-mode: its own client, so Tola answers Typst buffers without lsp-mode's Typst client.
(with-eval-after-load 'lsp-mode
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection '({arguments}))
    :initialization-options '(:packageSourceDirectory {packages})
    :activation-fn (lsp-activate-on "typst")
    :server-id 'tola-lsp)))
"#
    )
}

/// One Elisp string literal: every character stands for itself except the quote and backslash.
fn elisp_string(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        if character == '"' || character == '\\' {
            literal.push('\\');
        }
        literal.push(character);
    }
    literal.push('"');
    literal
}

#[cfg(test)]
mod tests {
    use super::super::tests::editor_directory;
    use super::*;

    #[test]
    fn clients_configure_package_navigation() {
        let snippet = snippet(&editor_directory("."));
        assert!(snippet.contains(r#"'(typst-ts-mode . ("tola" "lsp""#));
        let packages = elisp_string(&super::super::package_source_directory());
        for prefix in [":initializationOptions ", ":initialization-options '"] {
            assert!(snippet.contains(&format!("{prefix}(:packageSourceDirectory {packages})")));
        }
    }

    #[test]
    fn elisp_string_quotes_quote_and_backslash() {
        assert_eq!(elisp_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
