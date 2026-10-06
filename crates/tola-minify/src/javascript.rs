//! Script and module minification.

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions, LegalComment};
use oxc::mangler::MangleOptions;
use oxc::minifier::{CompressOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;
use thiserror::Error;

/// How the browser evaluates a JavaScript source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JavaScriptKind {
    /// A classic script.
    ///
    /// Top-level bindings keep their names: inline handlers and other scripts on the same page can
    /// reference them, and nothing in one script file proves those names unused.
    Classic,
    /// A module.
    ///
    /// Nothing outside the module can reference its top-level bindings, so they may be mangled.
    Module,
}

impl JavaScriptKind {
    fn source_type(self) -> SourceType {
        match self {
            Self::Classic => SourceType::script(),
            Self::Module => SourceType::mjs(),
        }
    }

    fn mangle_and_compress(self) -> (MangleOptions, CompressOptions) {
        match self {
            Self::Classic => (
                MangleOptions {
                    top_level: Some(false),
                    ..MangleOptions::default()
                },
                CompressOptions::safest(),
            ),
            Self::Module => (MangleOptions::default(), CompressOptions::smallest()),
        }
    }
}

/// A script or module this crate could not rewrite.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum JavaScriptMinifyError {
    /// The source could not be parsed, with the parser's own reason and position.
    #[error("{}", .reason)]
    Syntax {
        /// What the parser rejected, in its own words.
        reason: String,
        /// One-based line of the rejected syntax.
        line: u32,
        /// One-based column of the rejected syntax.
        column: u32,
    },
}

/// The one-based line and column of a byte offset in `source`.
fn position_of(source: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = source[line_start..offset].chars().count() + 1;
    (line as u32, column as u32)
}

/// Minify a script or module, preserving legal comments.
///
/// ```
/// # use tola_minify::{JavaScriptKind, minify_javascript};
/// let module = minify_javascript("export const answer = 6 * 7;", JavaScriptKind::Module).unwrap();
/// assert!(module.contains("answer"), "{module}");
/// ```
pub fn minify_javascript(
    source: &str,
    kind: JavaScriptKind,
) -> Result<String, JavaScriptMinifyError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, kind.source_type()).parse();
    if let Some(diagnostic) = parsed.diagnostics.errors().next() {
        let offset = diagnostic
            .labels
            .first()
            .map_or(0, |label| label.offset() as usize);
        let (line, column) = position_of(source, offset);
        return Err(JavaScriptMinifyError::Syntax {
            reason: diagnostic.message.to_string(),
            line,
            column,
        });
    }
    let mut program = parsed.program;
    let (mangle, compress) = kind.mangle_and_compress();
    let result = Minifier::new(MinifierOptions {
        mangle: Some(mangle),
        mangle_properties: None,
        compress: Some(compress),
    })
    .minify(&allocator, &mut program);
    Ok(Codegen::new()
        .with_options(CodegenOptions {
            minify: true,
            comments: CommentOptions {
                normal: false,
                jsdoc: false,
                annotation: false,
                legal: LegalComment::Inline,
            },
            ..CodegenOptions::default()
        })
        .with_scoping(result.scoping)
        .build(&program)
        .code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_keep_legal_comments() {
        let javascript = minify_javascript(
            "/*! Copyright */ function answer() { return 6 * 7; }",
            JavaScriptKind::Classic,
        )
        .unwrap();
        assert!(javascript.starts_with("/*! Copyright */"), "{javascript:?}");
    }

    #[test]
    fn scripts_keep_cross_script_bindings() {
        let script = minify_javascript(
            "function inlineHandler() { return crossScriptValue; } var crossScriptValue = 42;",
            JavaScriptKind::Classic,
        )
        .unwrap();
        assert!(script.contains("inlineHandler"), "{script}");
        assert!(script.contains("crossScriptValue"), "{script}");
    }

    #[test]
    fn modules_keep_their_exports() {
        let module =
            minify_javascript("export const answer = 6 * 7;", JavaScriptKind::Module).unwrap();
        assert!(module.contains("export"), "{module}");
        assert!(module.contains("answer"), "{module}");
    }

    #[test]
    fn script_syntax_error_names_position() {
        let JavaScriptMinifyError::Syntax {
            reason,
            line,
            column,
        } = minify_javascript("function =", JavaScriptKind::Classic).unwrap_err();
        assert!(!reason.is_empty(), "the parser's reason survives");
        assert_eq!((line, column), (1, 10));
    }
}
