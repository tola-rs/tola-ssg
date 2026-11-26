//! Stylesheet minification.

use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use thiserror::Error;

/// Error returned when a stylesheet cannot be minified.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum CssMinifyError {
    /// Stylesheet failed to parse.
    #[error("{}", .reason)]
    Syntax {
        /// Parser diagnostic message.
        reason: String,
        /// One-based line of the rejected syntax.
        line: u32,
        /// One-based column of the rejected syntax.
        column: u32,
    },
    /// Parsed stylesheet uses syntax the minifier cannot rewrite.
    #[error("the stylesheet uses syntax this minifier cannot rewrite")]
    Unsupported,
    /// Minified stylesheet could not be printed.
    #[error("the minified stylesheet could not be printed")]
    Print,
}

/// Minify a stylesheet, preserving legal comments (`/*! ... */`).
///
/// ```
/// # use tola_minify::minify_css;
/// assert_eq!(
///     minify_css(".a { color: red; } .b { color: red; }").unwrap(),
///     ".a,.b{color:red}"
/// );
/// ```
pub fn minify_css(src: &str) -> Result<String, CssMinifyError> {
    let mut sheet = StyleSheet::parse(src, ParserOptions::default()).map_err(|err| {
        // lightningcss numbers lines from zero and columns from one.
        let (line, column) = err
            .loc
            .as_ref()
            .map_or((1, 1), |loc| (loc.line + 1, loc.column));
        CssMinifyError::Syntax {
            reason: err.kind.to_string(),
            line,
            column,
        }
    })?;
    sheet
        .minify(MinifyOptions::default())
        .map_err(|_| CssMinifyError::Unsupported)?;
    sheet
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .map(|res| res.code)
        .map_err(|_| CssMinifyError::Print)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_comment_survives() {
        let css = minify_css("/*! Copyright */ .a { color: red; }").unwrap();
        assert!(css.starts_with("/*! Copyright */"), "{css:?}");
    }
}
