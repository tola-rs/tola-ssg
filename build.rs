//! Build script for minifying embedded JS/CSS assets and compiling welcome.typ.

use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc::mangler::MangleOptions;
use oxc::minifier::{CompressOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;
use std::fs;
use std::path::Path;
use typst_batch::{Compiler, WithInputs};

const HOTRELOAD_CSS_PLACEHOLDER: &str = "__TOLA_ERROR_OVERLAY_CSS__";

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);

    minify_js_file("src/embed/build/spa.js", &out_path.join("spa.min.js"));
    minify_hotreload_js_file(
        "src/embed/serve/hotreload.js",
        "src/embed/serve/hotreload-error-overlay.css",
        &out_path.join("hotreload.min.js"),
    );
    compile_welcome_typ(&out_path.join("welcome.html"));

    println!("cargo:rerun-if-changed=src/embed/build/spa.js");
    println!("cargo:rerun-if-changed=src/embed/serve/hotreload.js");
    println!("cargo:rerun-if-changed=src/embed/serve/hotreload-error-overlay.css");
    println!("cargo:rerun-if-changed=src/embed/serve/welcome.typ");
    println!("cargo:rerun-if-changed=src/embed/serve/welcome.css");
    println!("cargo:rerun-if-changed=src/embed/serve/tola.webp");
}

fn minify_js(source: &str) -> String {
    let allocator = Allocator::default();
    let source_type = SourceType::mjs();

    let ret = Parser::new(&allocator, source, source_type).parse();
    assert!(ret.errors.is_empty(), "Parse errors: {:?}", ret.errors);

    let mut program = ret.program;
    let options = MinifierOptions {
        mangle: Some(MangleOptions::default()),
        compress: Some(CompressOptions::smallest()),
    };
    let ret = Minifier::new(options).minify(&allocator, &mut program);

    Codegen::new()
        .with_options(CodegenOptions {
            minify: true,
            comments: CommentOptions::disabled(),
            ..CodegenOptions::default()
        })
        .with_scoping(ret.scoping)
        .build(&program)
        .code
}

fn minify_js_file(input: &str, output: &Path) {
    let source = fs::read_to_string(input).expect("Failed to read JS file");
    write_minified_js(&source, output, "Failed to write minified JS");
}

fn minify_hotreload_js_file(js_input: &str, css_input: &str, output: &Path) {
    let mut source = fs::read_to_string(js_input).expect("Failed to read hotreload.js");
    let css_source =
        fs::read_to_string(css_input).expect("Failed to read hotreload-error-overlay.css");
    let css = minify_css(&css_source);
    let escaped_css = escape_template_literal(&css);

    let count = source.matches(HOTRELOAD_CSS_PLACEHOLDER).count();
    assert_eq!(
        count, 1,
        "hotreload.js must contain exactly one {} placeholder",
        HOTRELOAD_CSS_PLACEHOLDER
    );

    source = source.replace(HOTRELOAD_CSS_PLACEHOLDER, &escaped_css);
    write_minified_js(&source, output, "Failed to write minified hotreload JS");
}

fn escape_template_literal(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

fn write_minified_js(source: &str, output: &Path, write_error: &str) {
    let code = minify_js(source);
    fs::write(output, code).expect(write_error);
}

fn minify_css(source: &str) -> String {
    let stylesheet =
        StyleSheet::parse(source, ParserOptions::default()).expect("Failed to parse CSS");
    stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..Default::default()
        })
        .expect("Failed to minify CSS")
        .code
}

#[allow(dead_code)]
fn minify_css_file(input: &str, output: &Path) {
    let source = fs::read_to_string(input).expect("Failed to read CSS file");
    let code = minify_css(&source);
    fs::write(output, code).expect("Failed to write minified CSS");
}

fn compile_welcome_typ(output: &Path) {
    let root = Path::new("src/embed/serve");
    let css_source =
        fs::read_to_string("src/embed/serve/welcome.css").expect("Failed to read welcome.css");
    let css = minify_css(&css_source);
    let result = Compiler::new(root)
        .with_inputs([("css", css.as_str())])
        .with_path(Path::new("welcome.typ"))
        .compile()
        .expect("Failed to compile welcome.typ");
    let html = result.html().expect("Failed to export HTML");
    fs::write(output, html).expect("Failed to write welcome.html");
}
