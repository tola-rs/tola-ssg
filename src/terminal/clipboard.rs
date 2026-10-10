//! The escape a terminal reads as the reader's own clipboard.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The escape that puts `text` on the terminal's clipboard (OSC 52, the system clipboard).
pub(crate) fn escape(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_carries_text_as_base64() {
        let escape = escape("命令 and spaces");

        assert!(escape.starts_with("\x1b]52;c;"), "{escape:?}");
        assert!(escape.ends_with('\x07'), "{escape:?}");
        let encoded = escape
            .trim_start_matches("\x1b]52;c;")
            .trim_end_matches('\x07');
        assert_eq!(
            STANDARD.decode(encoded).unwrap(),
            "命令 and spaces".as_bytes(),
            "the terminal reads the text back"
        );
    }
}
