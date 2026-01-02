//! Asset minification for JS and CSS files.
//!
//! Uses oxc for JavaScript and lightningcss for CSS.

use std::path::Path;

use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc::mangler::MangleOptions;
use oxc::minifier::{CompressOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;

/// Minify JavaScript source code.
pub fn minify_js(source: &str) -> Option<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::mjs();
    let ret = Parser::new(&allocator, source, source_type).parse();
    if !ret.errors.is_empty() {
        return None;
    }
    let mut program = ret.program;
    let options = MinifierOptions {
        mangle: Some(MangleOptions::default()),
        compress: Some(CompressOptions::smallest()),
    };
    let ret = Minifier::new(options).minify(&allocator, &mut program);
    let code = Codegen::new()
        .with_options(CodegenOptions {
            minify: true,
            comments: CommentOptions::disabled(),
            ..CodegenOptions::default()
        })
        .with_scoping(ret.scoping)
        .build(&program)
        .code;
    Some(code)
}

/// Minify CSS source code.
pub fn minify_css(source: &str) -> Option<String> {
    let stylesheet = StyleSheet::parse(source, ParserOptions::default()).ok()?;
    let result = stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .ok()?;
    Some(result.code)
}

/// Minify content based on file extension.
///
/// Returns `Some(minified)` if minification succeeded, `None` otherwise.
pub fn minify_by_ext(path: &Path, content: &str) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "js" => minify_js(content),
        "css" => minify_css(content),
        _ => None,
    }
}
