//! Stylesheet minification.

use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
use thiserror::Error;

/// A stylesheet this crate could not rewrite.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum CssMinifyError {
    /// The stylesheet could not be parsed, with the parser's own reason and position.
    #[error("{}", .reason)]
    Syntax {
        /// What the parser rejected, in its own words.
        reason: String,
        /// One-based line of the rejected syntax.
        line: u32,
        /// One-based column of the rejected syntax.
        column: u32,
    },
    /// The parsed stylesheet uses syntax the semantic minifier cannot rewrite.
    #[error("the stylesheet uses syntax this minifier cannot rewrite")]
    Unsupported,
    /// The minified stylesheet could not be printed.
    #[error("the minified stylesheet could not be printed")]
    Print,
}

/// Minify a stylesheet, preserving legal comments.
///
/// ```
/// # use tola_minify::minify_css;
/// assert_eq!(
///     minify_css(".a { color: red; } .b { color: red; }").unwrap(),
///     ".a,.b{color:red}"
/// );
/// ```
pub fn minify_css(source: &str) -> Result<String, CssMinifyError> {
    let mut stylesheet = StyleSheet::parse(source, ParserOptions::default()).map_err(|error| {
        // The parser numbers lines from zero and columns from one.
        let (line, column) = error
            .loc
            .as_ref()
            .map_or((1, 1), |location| (location.line + 1, location.column));
        CssMinifyError::Syntax {
            reason: error.kind.to_string(),
            line,
            column,
        }
    })?;
    stylesheet
        .minify(MinifyOptions::default())
        .map_err(|_| CssMinifyError::Unsupported)?;
    stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .map(|result| result.code)
        .map_err(|_| CssMinifyError::Print)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_rules_merge_into_one() {
        assert_eq!(
            minify_css(".a { color: red; } .b { color: red; }").unwrap(),
            ".a,.b{color:red}"
        );
    }

    #[test]
    fn css_keeps_legal_comments() {
        let css = minify_css("/*! Copyright */ .a { color: red; }").unwrap();
        assert!(css.starts_with("/*! Copyright */"), "{css:?}");
    }

    #[test]
    fn css_syntax_error_names_position() {
        let error = minify_css("@media (").unwrap_err();
        let CssMinifyError::Syntax {
            reason,
            line,
            column,
        } = error
        else {
            panic!("expected a syntax error");
        };
        assert!(!reason.is_empty(), "the parser's reason survives");
        assert_eq!((line, column), (1, 9));
    }
}
