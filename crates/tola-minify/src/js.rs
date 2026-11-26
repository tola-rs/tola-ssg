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
pub enum JsKind {
    /// Classic script whose top-level bindings keep their names.
    Classic,
    /// ES module whose non-exported top-level bindings may be mangled.
    Module,
}

impl JsKind {
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
                    // Inline handlers and sibling scripts may reference top-level names.
                    top_level: Some(false),
                    ..MangleOptions::default()
                },
                CompressOptions::safest(),
            ),
            Self::Module => (MangleOptions::default(), CompressOptions::smallest()),
        }
    }
}

/// Error returned when a script or module cannot be minified.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum JsMinifyError {
    /// Source failed to parse.
    #[error("{}", .reason)]
    Syntax {
        /// Parser diagnostic message.
        reason: String,
        /// One-based line of the rejected syntax.
        line: u32,
        /// One-based column of the rejected syntax.
        column: u32,
    },
}

fn line_col(src: &str, offset: usize) -> (u32, u32) {
    let pos = offset.min(src.len());
    let before = &src[..pos];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |idx| idx + 1);
    let col = src[line_start..pos].chars().count() + 1;
    (line as u32, col as u32)
}

/// Minify a script or module, preserving legal comments (`/*! ... */`).
///
/// ```
/// # use tola_minify::{JsKind, minify_js};
/// let out = minify_js("export const answer = 6 * 7;", JsKind::Module).unwrap();
/// assert!(out.contains("answer"), "{out}");
/// ```
pub fn minify_js(src: &str, kind: JsKind) -> Result<String, JsMinifyError> {
    let alloc = Allocator::default();
    let parsed = Parser::new(&alloc, src, kind.source_type()).parse();
    if let Some(diag) = parsed.diagnostics.errors().next() {
        let offset = diag.labels.first().map_or(0, |lbl| lbl.offset() as usize);
        let (line, column) = line_col(src, offset);
        return Err(JsMinifyError::Syntax {
            reason: diag.message.to_string(),
            line,
            column,
        });
    }
    let mut program = parsed.program;
    let (mangle, compress) = kind.mangle_and_compress();
    let res = Minifier::new(MinifierOptions {
        mangle: Some(mangle),
        mangle_properties: None,
        compress: Some(compress),
    })
    .minify(&alloc, &mut program);
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
        .with_scoping(res.scoping)
        .build(&program)
        .code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_keeps_legal_comment() {
        let out = minify_js(
            "/*! Copyright */ function answer() { return 6 * 7; }",
            JsKind::Classic,
        )
        .unwrap();
        assert!(out.starts_with("/*! Copyright */"), "{out:?}");
    }

    #[test]
    fn syntax_error_location() {
        let JsMinifyError::Syntax {
            reason,
            line,
            column,
        } = minify_js("function =", JsKind::Classic).unwrap_err();
        assert!(!reason.is_empty());
        assert_eq!((line, column), (1, 10));
    }
}
