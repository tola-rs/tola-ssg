//! Syntax highlighting shared by Tola's terminal documentation and Nu previews.
//!
//! Uses Typst's built-in syntax definitions and shares extra definitions with `@tola/code`.

use std::ops::Range;
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Highlighter, Style, Theme};
use syntect::parsing::{Scope, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use typst_library::text::RAW_SYNTAXES;
use typst_syntax::{LinkedNode, SyntaxKind};

mod signature;
mod theme;
pub use theme::ThemeChoice;

/// Extra syntax files also available to Typst's `raw.syntaxes`, keyed by language.
pub const CODE_SYNTAXES: &[(&str, &str)] = &[
    ("cuda", include_str!("syntaxes/cuda.sublime-syntax")),
    ("fennel", include_str!("syntaxes/fennel.sublime-syntax")),
    ("just", include_str!("syntaxes/just.sublime-syntax")),
    ("lean4", include_str!("syntaxes/lean4.sublime-syntax")),
    ("nushell", include_str!("syntaxes/nushell.sublime-syntax")),
    ("scheme", include_str!("syntaxes/scheme.sublime-syntax")),
];

/// Map a source-code fence label to the name of its built-in Typst syntax.
pub fn code_language(lang: &str) -> &str {
    match lang.to_ascii_lowercase().as_str() {
        "common-lisp" | "sbcl" | "ccl" | "clozure" | "clozure-cl" | "ecl" | "abcl" | "cmucl"
        | "clasp" | "lispworks" | "elisp" | "emacs-lisp" => "lisp",
        "clojurescript" => "cljs",
        "babashka" => "clj",
        "csharp" => "C#",
        "fsharp" => "F#",
        "docker" => "dockerfile",
        "golang" => "go",
        "python3" => "python",
        _ => lang,
    }
}

static EXTRA_SYNTAXES: LazyLock<Vec<SyntaxSet>> = LazyLock::new(|| {
    // Typst loads each raw.syntaxes file in its own set.
    CODE_SYNTAXES
        .iter()
        .copied()
        .map(|(name, src)| {
            let mut builder = SyntaxSetBuilder::new();
            builder.add(
                SyntaxDefinition::load_from_str(src, false, None)
                    .unwrap_or_else(|err| panic!("invalid bundled {name} syntax: {err}")),
            );
            builder.build()
        })
        .collect()
});

/// Whether a bundled syntax recognizes this language name or extension.
pub fn has_bundled_syntax(lang: &str) -> bool {
    EXTRA_SYNTAXES
        .iter()
        .any(|set| set.find_syntax_by_token(lang).is_some())
}

/// Styles over the original source, preserving layout and source offsets.
pub struct Highlighted<'a> {
    src: &'a str,
    runs: Vec<(Range<usize>, Style)>,
    terminal_colors: bool,
}

impl Highlighted<'_> {
    /// Append a UTF-8 byte range within the original source as ANSI text.
    pub fn paint(&self, range: Range<usize>, out: &mut String) {
        let mut off = range.start;
        let first = self.runs.partition_point(|(span, _)| span.end <= off);
        for (span, style) in &self.runs[first..] {
            if span.start >= range.end {
                break;
            }
            let start = span.start.max(range.start);
            let end = span.end.min(range.end);
            out.push_str(&self.src[off..start]);
            push_ansi(out, *style, &self.src[start..end], self.terminal_colors);
            off = end;
        }
        out.push_str(&self.src[off..range.end]);
    }
}

/// Compute terminal styles for a fence tag or file extension.
pub fn highlight<'a>(src: &'a str, lang: &str, choice: ThemeChoice) -> Highlighted<'a> {
    let mut runs = Vec::new();
    let terminal_colors = choice == ThemeChoice::Terminal;
    if let Some(th) = theme::theme_of(choice) {
        let hl = Highlighter::new(th);
        collect(src, lang, th, &hl, 0, &mut runs);
    }
    Highlighted {
        src,
        runs,
        terminal_colors,
    }
}

/// Compute terminal styles for a Typst API signature or value declaration.
pub fn highlight_typst_signature<'a>(src: &'a str, choice: ThemeChoice) -> Highlighted<'a> {
    let mut runs = Vec::new();
    let terminal_colors = choice == ThemeChoice::Terminal;
    if let Some(th) = theme::theme_of(choice) {
        let hl = Highlighter::new(th);
        signature::collect(src, th, &hl, &mut runs);
    }
    Highlighted {
        src,
        runs,
        terminal_colors,
    }
}

/// Sanitize source for a terminal preview and paint the whole result.
pub fn render(src: &str, lang: &str, choice: ThemeChoice) -> String {
    let src = sanitize(src);
    let hl = highlight(&src, lang, choice);
    let mut out = String::with_capacity(src.len());
    hl.paint(0..src.len(), &mut out);
    out
}

fn collect(
    src: &str,
    lang: &str,
    th: &Theme,
    hl: &Highlighter<'_>,
    off: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let lang = lang.trim().to_ascii_lowercase();
    let root = match lang.as_str() {
        "typ" | "typst" => typst_syntax::parse(src),
        "typc" | "typst-code" => typst_syntax::parse_code(src),
        "typm" | "typst-math" => typst_syntax::parse_math(src),
        _ => {
            collect_syntect(src, &lang, th, off, runs);
            return;
        }
    };
    walk_typst(
        &LinkedNode::new(&root),
        src,
        th,
        hl,
        &mut Vec::new(),
        off,
        runs,
        false,
    );
}

fn collect_syntect(
    src: &str,
    lang: &str,
    th: &Theme,
    mut off: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let tok = match lang {
        "shell" | "console" => "sh",
        _ => code_language(lang),
    };
    let Some((syntaxes, syntax)) = EXTRA_SYNTAXES
        .iter()
        .chain([&*RAW_SYNTAXES])
        .find_map(|set| set.find_syntax_by_token(tok).map(|s| (set, s)))
    else {
        return;
    };
    let mut lines = HighlightLines::new(syntax, th);
    for line in src.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        if let Ok(spans) = lines.highlight_line(text, syntaxes) {
            let mut start = off;
            for (style, piece) in spans {
                let end = start + piece.len();
                if start < end {
                    runs.push((start..end, style));
                }
                start = end;
            }
        }
        off += line.len();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn walk_typst(
    node: &LinkedNode<'_>,
    src: &str,
    th: &Theme,
    hl: &Highlighter<'_>,
    scopes: &mut Vec<Scope>,
    off: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
    skip_invalid: bool,
) {
    let scope = typst_syntax::highlight(node)
        .filter(|tag| !skip_invalid || !tag.tm_scope().starts_with("invalid"))
        .and_then(|tag| Scope::new(tag.tm_scope()).ok());
    scopes.extend(scope);

    if node.kind() == SyntaxKind::Raw
        && let Some(lang) = node
            .cast::<typst_syntax::ast::Raw>()
            .and_then(|raw| raw.lang())
        && let Some(body) = raw_body(node)
    {
        let span = node.range();
        let style = hl.style_for_stack(scopes);
        runs.push((off + span.start..off + body.start, style));
        collect(
            &src[body.clone()],
            lang.get(),
            th,
            hl,
            off + body.start,
            runs,
        );
        runs.push((off + body.end..off + span.end, style));
    } else if node.children().len() == 0 {
        let span = node.range();
        if !span.is_empty() && !scopes.is_empty() {
            runs.push((off + span.start..off + span.end, hl.style_for_stack(scopes)));
        }
    } else {
        for child in node.children() {
            walk_typst(&child, src, th, hl, scopes, off, runs, skip_invalid);
        }
    }

    if scope.is_some() {
        scopes.pop();
    }
}

fn raw_body(node: &LinkedNode<'_>) -> Option<Range<usize>> {
    let mut text = node
        .children()
        .filter(|child| child.kind() == SyntaxKind::Text);
    let first = text.next()?.range();
    let end = text.next_back().map_or(first.end, |c| c.range().end);
    Some(first.start..end)
}

fn push_ansi(out: &mut String, style: Style, text: &str, terminal_colors: bool) {
    if text.is_empty() {
        return;
    }
    let fg = style.foreground;
    // Terminal palette encodes SGR fg codes in the blue channel of syntect's RGB Color.
    let mut codes = if terminal_colors {
        fg.b.to_string()
    } else {
        format!("38;2;{};{};{}", fg.r, fg.g, fg.b)
    };
    let font = style.font_style;
    if font.contains(FontStyle::BOLD) {
        codes.push_str(";1");
    }
    if font.contains(FontStyle::ITALIC) {
        codes.push_str(";3");
    }
    if font.contains(FontStyle::UNDERLINE) {
        codes.push_str(";4");
    }
    out.push_str("\x1b[");
    out.push_str(&codes);
    out.push('m');
    out.push_str(text);
    out.push_str("\x1b[0m");
}

/// Strip terminal control sequences and expand tabs before painting.
fn sanitize(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('\x40'..='\x7e').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\t' => out.push_str("    "),
            '\r' => {}
            ch if ch.is_control() && ch != '\n' => {}
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_highlighted() {
        let ansi = render(
            "fn main() {\n    let x = 1;\n}\n",
            "rust",
            ThemeChoice::Dark,
        );
        assert!(ansi.contains("\x1b[38;2;"));
        assert!(ansi.contains("fn"));
        assert!(ansi.contains("let"));
        assert!(ansi.contains("}"));
        assert!(ansi.ends_with('\n'));
    }

    #[test]
    fn unknown_lang_sanitizes() {
        let ansi = render(
            "a\tb\x1b[31mred\x1b[0m",
            "not-a-language",
            ThemeChoice::Dark,
        );
        assert_eq!(ansi, "a    bred");
    }

    #[test]
    fn plain_theme_uncolored() {
        let ansi = render("fn main() {}", "rust", ThemeChoice::Plain);
        assert_eq!(ansi, "fn main() {}");
    }

    #[test]
    fn typst_tags_painted() {
        let ansi = render("#let x = 1\n= Heading\n", "typ", ThemeChoice::Dark);
        assert!(ansi.contains("\x1b[38;2;"));
        assert!(ansi.contains("Heading"));
    }

    #[test]
    fn typst_code_fence() {
        let src = "let x = 1";
        assert!(highlight(src, "typ", ThemeChoice::Dark).runs.is_empty());
        assert!(!highlight(src, "typc", ThemeChoice::Dark).runs.is_empty());
    }

    #[test]
    fn just_recipe_and_interp() {
        let src = "mode := \"release\"\ncheck: prepare\n    cargo check --profile {{mode}}\n";
        let code = highlight(src, "justfile", ThemeChoice::Dark);
        let style_at = |idx| {
            code.runs
                .iter()
                .find(|(span, _)| span.contains(&idx))
                .map(|(_, style)| *style)
        };
        let cmd = style_at(src.find("cargo").unwrap());
        assert!(cmd.is_some());
        assert_eq!(cmd, style_at(src.find("--profile").unwrap()));
        for idx in [
            src.find("release").unwrap(),
            src.find("check:").unwrap(),
            src.find("prepare").unwrap(),
            src.rfind("mode").unwrap(),
        ] {
            let style = style_at(idx);
            assert!(style.is_some());
            assert_ne!(style, cmd);
        }
    }

    #[test]
    fn extra_fence_tags() {
        let cases = [
            ("lean4", "def twice (x : Nat) := x + x"),
            ("chez-scheme", "(define (twice x) (+ x x))"),
            ("r7rs", "(define-library (demo))"),
            ("fnl", "(fn twice [x] (+ x x))"),
            ("nu", "let x = 2"),
            ("cu", "kernel<<<grid, block>>>(data);"),
        ];
        for (tag, src) in cases {
            assert!(
                !highlight(src, tag, ThemeChoice::Dark).runs.is_empty(),
                "{tag}"
            );
        }
        assert!(
            EXTRA_SYNTAXES
                .iter()
                .all(|set| set.find_syntax_by_token("rkt").is_none())
        );
    }

    #[test]
    fn nu_cmd_and_flags() {
        let src = "use tola [get-context get-site]\n(get-context | save --raw)\n1 - 2\n";
        let code = highlight(src, "nu", ThemeChoice::Dark);
        let style = |start, len| {
            code.runs
                .iter()
                .find(|(span, _)| span.start <= start && start + len <= span.end)
                .map(|(_, style)| *style)
                .expect("token has a single style")
        };
        let minus = style(src.find(" - ").unwrap() + 1, 1);
        assert_ne!(style(src.find("get-context").unwrap(), 11), minus);
        assert_ne!(style(src.rfind("get-context").unwrap(), 11), minus);
        assert_ne!(style(src.find("--raw").unwrap(), 5), minus);
    }

    #[test]
    fn nu_token_context() {
        use syntect::parsing::{ParseState, ScopeStack};

        let (syntaxes, syntax) = EXTRA_SYNTAXES
            .iter()
            .find_map(|set| set.find_syntax_by_token("nu").map(|s| (set, s)))
            .unwrap();
        let mut parser = ParseState::new(syntax);
        let mut scopes = ScopeStack::new();
        let lines: &[(&str, &[(&str, &str)])] = &[
            (
                "def inspect [--output(-l): string = short, rows: list<record<name: string>>]: nothing -> any {",
                &[
                    ("--output", "constant.other.option.nu"),
                    ("-l", "constant.other.option.nu"),
                    ("short", "source.nu"),
                    ("list", "storage.type.nu"),
                    ("name", "variable.other.member.nu"),
                    ("->", "keyword.operator.type.nu"),
                    ("any", "storage.type.nu"),
                ],
            ),
            (
                "  let data = {sample: 1..=3}",
                &[
                    ("sample", "variable.other.member.nu"),
                    ("..=", "keyword.operator.range.nu"),
                ],
            ),
            (
                "  match $data.sample!? { 1 | 2 => true, _ => false }",
                &[
                    ("!?", "keyword.operator.member.nu"),
                    ("|", "keyword.operator.pattern.nu"),
                    ("=>", "keyword.operator.match.nu"),
                    ("_", "constant.language.wildcard.nu"),
                ],
            ),
            ("}", &[]),
            (
                r#"LABEL="test" %echo and -- --raw e>| save --raw"#,
                &[
                    ("LABEL", "variable.other.environment.nu"),
                    ("%", "keyword.operator.command.nu"),
                    ("echo", "variable.function.nu"),
                    ("and", "source.nu"),
                    ("--raw", "source.nu"),
                    ("e>|", "keyword.operator.pipe.nu"),
                    ("save", "variable.function.nu"),
                ],
            ),
        ];
        for &(line, expected) in lines {
            let ops = parser.parse_line(line, syntaxes).unwrap();
            for &(tok, name) in expected {
                let start = line.find(tok).unwrap();
                let end = start + tok.len() - 1;
                let scope = Scope::new(name).unwrap();
                for off in [start, end] {
                    let mut stack = scopes.clone();
                    for (_, op) in ops.iter().take_while(|(at, _)| *at <= off) {
                        stack.apply(op).unwrap();
                    }
                    assert_eq!(stack.as_slice().last(), Some(&scope), "{line}: {tok}");
                }
            }
            for (_, op) in ops {
                scopes.apply(&op).unwrap();
            }
        }
    }

    #[test]
    fn builtin_aliases() {
        let cases = [
            ("SBCL", "lisp", "(defun twice (x) (+ x x))"),
            ("ccl", "lisp", "(defun twice (x) (+ x x))"),
            ("babashka", "clj", "(defn twice [x] (+ x x))"),
            ("fsharp", "F#", "let x = 2"),
        ];
        for (alias, builtin, src) in cases {
            let expected = highlight(src, builtin, ThemeChoice::Dark);
            assert!(!expected.runs.is_empty(), "{builtin}");
            assert_eq!(
                highlight(src, alias, ThemeChoice::Dark).runs,
                expected.runs,
                "{alias}"
            );
        }
    }

    #[test]
    fn typst_raw_embedded_rust() {
        let ansi = render(
            "\u{60}\u{60}\u{60}rust\nfn main() {}\n\u{60}\u{60}\u{60}\n",
            "typ",
            ThemeChoice::Dark,
        );
        assert!(ansi.contains("fn"));
        assert!(ansi.contains("38;2;255;123;114"));
    }

    #[test]
    fn theme_names_parse() {
        assert_eq!(ThemeChoice::parse("DARK"), Some(ThemeChoice::Dark));
        assert_eq!(ThemeChoice::parse("plain"), Some(ThemeChoice::Plain));
        assert_eq!(ThemeChoice::parse("nope"), None);
    }
}
