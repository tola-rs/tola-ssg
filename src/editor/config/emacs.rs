//! Emacs language-server registration for Eglot and lsp-mode, as an init-file snippet.

use super::EditorDirectory;

/// Both clients are configured in the author's init file: Eglot's `eglot-server-programs` and
/// lsp-mode's client registry are user options, and neither reads a site file. The two languages
/// the server answers are one major mode each here — `typst-ts-mode` opens the Typst sources, and
/// the site's configuration stays the TOML mode's — and the language id both clients send is the
/// one Tola answers, `typst`, derived from the mode and from `lsp-activate-on` respectively.
/// `eglot-server-programs` pairs a mode with its program and arguments, so the entry is the dotted
/// pair `(typst-ts-mode . ("tola" …))`, the list Eglot reads as the contact.
pub(super) fn snippet(directory: &EditorDirectory) -> String {
    let command = directory.server_command();
    let mut words = vec![elisp_string(command.command)];
    words.extend(
        command
            .arguments
            .iter()
            .map(|argument| elisp_string(argument)),
    );
    let command = format!("({})", words.join(" "));
    format!(
        r#";; Eglot (Emacs 29+), in the author's init file.
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '(typst-ts-mode . {command})))

;; lsp-mode: its own client, so Tola answers Typst buffers without lsp-mode's Typst client.
(with-eval-after-load 'lsp-mode
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection '{command})
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
    fn eglot_entry_passes_the_site_arguments() {
        let snippet = snippet(&editor_directory("."));
        assert!(
            snippet.contains(r#"'(typst-ts-mode . ("tola" "lsp" "--config" "./tola.toml"))"#),
            "Eglot reads the entry as the mode paired with the command list: {snippet}"
        );
    }

    #[test]
    fn elisp_string_quotes_quote_and_backslash() {
        assert_eq!(elisp_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
