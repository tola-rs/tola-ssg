//! Syntax highlighting shared by Tola's terminal documentation and Nu previews.
//!
//! Uses Typst's built-in syntax definitions and shares extra definitions with `@tola/code`.

use std::ops::Range;
use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, Highlighter, Style, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{Scope, SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};
use typst_library::text::RAW_SYNTAXES;
use typst_syntax::{LinkedNode, SyntaxKind};

/// The extra syntax files also available to Typst's `raw.syntaxes`, keyed by language.
pub const CODE_SYNTAXES: &[(&str, &str)] = &[
    ("fennel", include_str!("syntaxes/fennel.sublime-syntax")),
    ("just", include_str!("syntaxes/just.sublime-syntax")),
    ("lean4", include_str!("syntaxes/lean4.sublime-syntax")),
    ("nushell", include_str!("syntaxes/nushell.sublime-syntax")),
    ("scheme", include_str!("syntaxes/scheme.sublime-syntax")),
];

/// Map a source-code fence label to the name of its built-in Typst syntax.
pub fn code_language(language: &str) -> &str {
    match language.to_ascii_lowercase().as_str() {
        "common-lisp" | "sbcl" | "ccl" | "clozure" | "clozure-cl" | "ecl" | "abcl" | "cmucl"
        | "clasp" | "lispworks" | "elisp" | "emacs-lisp" => "lisp",
        "clojurescript" => "cljs",
        "babashka" => "clj",
        "csharp" => "C#",
        "fsharp" => "F#",
        "docker" => "dockerfile",
        "golang" => "go",
        "python3" => "python",
        _ => language,
    }
}

/// The palette used to paint a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    /// A dark palette for the default TUI surface.
    Dark,
    /// A light palette for terminal previews.
    Light,
    /// No ANSI escapes at all; the source is returned sanitized and tab-expanded.
    Plain,
}

impl ThemeChoice {
    /// Parse a theme name. Unknown names return `None` so the caller can decide.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "plain" | "none" | "never" => Some(Self::Plain),
            _ => None,
        }
    }

    /// The canonical name of this choice.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Plain => "plain",
        }
    }
}

static EXTRA_SYNTAXES: LazyLock<Vec<SyntaxSet>> = LazyLock::new(|| {
    // Typst loads each raw.syntaxes file in its own set.
    CODE_SYNTAXES
        .iter()
        .copied()
        .map(|(name, source)| {
            let mut builder = SyntaxSetBuilder::new();
            builder.add(
                SyntaxDefinition::load_from_str(source, false, None)
                    .unwrap_or_else(|error| panic!("invalid bundled {name} syntax: {error}")),
            );
            builder.build()
        })
        .collect()
});
static DARK: LazyLock<Theme> = LazyLock::new(|| palette(true));
static LIGHT: LazyLock<Theme> = LazyLock::new(|| palette(false));

/// Styles over the original source. Layout and source offsets are preserved.
pub struct Highlighted<'a> {
    source: &'a str,
    runs: Vec<(Range<usize>, Style)>,
}

impl Highlighted<'_> {
    /// Append a UTF-8 byte range as ANSI text. The range must be within the original source.
    pub fn paint(&self, range: Range<usize>, out: &mut String) {
        let mut offset = range.start;
        let first = self.runs.partition_point(|(span, _)| span.end <= offset);
        for (span, style) in &self.runs[first..] {
            if span.start >= range.end {
                break;
            }
            let start = span.start.max(range.start);
            let end = span.end.min(range.end);
            out.push_str(&self.source[offset..start]);
            push_ansi(out, *style, &self.source[start..end]);
            offset = end;
        }
        out.push_str(&self.source[offset..range.end]);
    }
}

/// Find terminal styles for a fence tag or file extension. Unknown languages read plain.
pub fn highlight<'a>(source: &'a str, language: &str, theme: ThemeChoice) -> Highlighted<'a> {
    let mut runs = Vec::new();
    if let Some(theme) = theme_of(theme) {
        let highlighter = Highlighter::new(theme);
        collect(source, language, theme, &highlighter, 0, &mut runs);
    }
    Highlighted { source, runs }
}

/// Sanitize source for a terminal preview and paint the whole result.
pub fn render(source: &str, language: &str, theme: ThemeChoice) -> String {
    let source = sanitize(source);
    let highlighted = highlight(&source, language, theme);
    let mut out = String::with_capacity(source.len());
    highlighted.paint(0..source.len(), &mut out);
    out
}

fn theme_of(choice: ThemeChoice) -> Option<&'static Theme> {
    match choice {
        ThemeChoice::Dark => Some(&DARK),
        ThemeChoice::Light => Some(&LIGHT),
        ThemeChoice::Plain => None,
    }
}

fn collect(
    source: &str,
    language: &str,
    theme: &Theme,
    highlighter: &Highlighter<'_>,
    offset: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let language = language.trim().to_ascii_lowercase();
    let root = match language.as_str() {
        "typ" | "typst" => typst_syntax::parse(source),
        "typc" | "typst-code" => typst_syntax::parse_code(source),
        "typm" | "typst-math" => typst_syntax::parse_math(source),
        _ => {
            collect_syntect(source, &language, theme, offset, runs);
            return;
        }
    };
    walk_typst(
        &LinkedNode::new(&root),
        source,
        theme,
        highlighter,
        &mut Vec::new(),
        offset,
        runs,
    );
}

fn collect_syntect(
    source: &str,
    language: &str,
    theme: &Theme,
    mut offset: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let token = match language {
        "shell" | "console" => "sh",
        _ => code_language(language),
    };
    let Some((syntaxes, syntax)) = EXTRA_SYNTAXES
        .iter()
        .chain([&*RAW_SYNTAXES])
        .find_map(|set| set.find_syntax_by_token(token).map(|syntax| (set, syntax)))
    else {
        return;
    };
    let mut lines = HighlightLines::new(syntax, theme);
    for line in source.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if let Ok(spans) = lines.highlight_line(text, syntaxes) {
            let mut start = offset;
            for (style, piece) in spans {
                let end = start + piece.len();
                if start < end {
                    runs.push((start..end, style));
                }
                start = end;
            }
        }
        offset += line.len();
    }
}

fn walk_typst(
    node: &LinkedNode<'_>,
    source: &str,
    theme: &Theme,
    highlighter: &Highlighter<'_>,
    scopes: &mut Vec<Scope>,
    offset: usize,
    runs: &mut Vec<(Range<usize>, Style)>,
) {
    let scope = typst_syntax::highlight(node).and_then(|tag| Scope::new(tag.tm_scope()).ok());
    scopes.extend(scope);

    if node.kind() == SyntaxKind::Raw
        && let Some(lang) = node
            .cast::<typst_syntax::ast::Raw>()
            .and_then(|raw| raw.lang())
        && let Some(body) = raw_body(node)
    {
        let range = node.range();
        let style = highlighter.style_for_stack(scopes);
        runs.push((offset + range.start..offset + body.start, style));
        collect(
            &source[body.clone()],
            lang.get(),
            theme,
            highlighter,
            offset + body.start,
            runs,
        );
        runs.push((offset + body.end..offset + range.end, style));
    } else if node.children().len() == 0 {
        let range = node.range();
        if !range.is_empty() && !scopes.is_empty() {
            runs.push((
                offset + range.start..offset + range.end,
                highlighter.style_for_stack(scopes),
            ));
        }
    } else {
        for child in node.children() {
            walk_typst(&child, source, theme, highlighter, scopes, offset, runs);
        }
    }

    if scope.is_some() {
        scopes.pop();
    }
}

/// The source between the first and last text nodes of a tagged raw element.
fn raw_body(node: &LinkedNode<'_>) -> Option<Range<usize>> {
    let mut text = node
        .children()
        .filter(|child| child.kind() == SyntaxKind::Text);
    let first = text.next()?.range();
    let end = text
        .next_back()
        .map_or(first.end, |child| child.range().end);
    Some(first.start..end)
}

fn push_ansi(out: &mut String, style: Style, text: &str) {
    if text.is_empty() {
        return;
    }
    let fg = style.foreground;
    let mut codes = format!("38;2;{};{};{}", fg.r, fg.g, fg.b);
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
fn sanitize(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
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
            character if character.is_control() && character != '\n' => {}
            character => out.push(character),
        }
    }
    out
}

fn color(rgb: u32) -> Option<Color> {
    Some(Color {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: 0xff,
    })
}

fn item(scope: &str, foreground: Option<u32>, font_style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: scope.parse().expect("theme scope parses"),
        style: syntect::highlighting::StyleModifier {
            foreground: foreground.and_then(color),
            background: None,
            font_style,
        },
    }
}

fn palette(dark: bool) -> Theme {
    let (
        foreground,
        comment,
        escape,
        delimiter,
        heading,
        marker,
        label,
        keyword,
        constant,
        string,
        function,
        macro_,
        annotation,
        other,
        diff,
        inserted,
        deleted,
        key,
        value,
        invalid,
    ) = if dark {
        (
            0xc9d1d9, 0x8b949e, 0x56d4dd, 0x56d4dd, 0xe6edf3, 0xd2a8ff, 0x79c0ff, 0xff7b72,
            0xffa657, 0xa5d6a7, 0x79c0ff, 0x79c0ff, 0xffa657, 0xd2a8ff, 0xd2a8ff, 0x7ee787,
            0xff7b72, 0x79c0ff, 0xa5d6a7, 0xff7b72,
        )
    } else {
        (
            0x24292f, 0x74747c, 0x1d6c76, 0x1d6c76, 0x1f2328, 0x8b41b1, 0x1d6c76, 0xd73948,
            0xb60157, 0x198810, 0x4b69c6, 0x16718d, 0x301414, 0x8b41b1, 0x8b41b1, 0x198810,
            0xd73948, 0x4b69c6, 0x198810, 0xd73948,
        )
    };
    let bold = Some(FontStyle::BOLD);
    let italic = Some(FontStyle::ITALIC);
    let underline = Some(FontStyle::UNDERLINE);
    Theme {
        name: Some(if dark {
            "Tola Dark".into()
        } else {
            "Tola Light".into()
        }),
        author: Some("The Tola Project Developers".into()),
        settings: ThemeSettings {
            foreground: color(foreground),
            background: None,
            ..Default::default()
        },
        scopes: vec![
            item("comment", Some(comment), None),
            item("constant.character.escape", Some(escape), None),
            item("markup.bold", None, bold),
            item("markup.italic", None, italic),
            item("markup.underline", None, underline),
            item("markup.raw", Some(comment), None),
            item("string.other.math.typst", None, None),
            item("punctuation.definition.math", Some(delimiter), None),
            item(
                "keyword.operator.math, punctuation.math.typst",
                Some(delimiter),
                None,
            ),
            item("markup.heading, entity.name.section", Some(heading), bold),
            item(
                "markup.heading.typst",
                Some(heading),
                Some(FontStyle::BOLD | FontStyle::UNDERLINE),
            ),
            item("punctuation.definition.list", Some(marker), None),
            item("markup.list.term", None, bold),
            item(
                "entity.name.label, markup.other.reference",
                Some(label),
                None,
            ),
            item(
                "keyword, constant.language, variable.language",
                Some(keyword),
                None,
            ),
            item("storage.type, storage.modifier", Some(keyword), None),
            item("constant", Some(constant), None),
            item("string", Some(string), None),
            item(
                "entity.name, variable.function, support",
                Some(function),
                None,
            ),
            item("support.macro", Some(macro_), None),
            item("meta.annotation", Some(annotation), None),
            item("entity.other, meta.interpolation", Some(other), None),
            item("meta.diff.range", Some(diff), None),
            item(
                "markup.inserted, meta.diff.header.to-file",
                Some(inserted),
                None,
            ),
            item(
                "markup.deleted, meta.diff.header.from-file",
                Some(deleted),
                None,
            ),
            item(
                "meta.mapping.key.json string.quoted.double.json",
                Some(key),
                None,
            ),
            item(
                "meta.mapping.value.json string.quoted.double.json",
                Some(value),
                None,
            ),
            item("invalid, invalid.typst", Some(invalid), underline),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_is_highlighted() {
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
    fn unknown_language_is_plain_but_sanitized() {
        let ansi = render(
            "a\tb\x1b[31mred\x1b[0m",
            "not-a-language",
            ThemeChoice::Dark,
        );
        assert_eq!(ansi, "a    bred");
    }

    #[test]
    fn plain_theme_has_no_ansi() {
        let ansi = render("fn main() {}", "rust", ThemeChoice::Plain);
        assert_eq!(ansi, "fn main() {}");
    }

    #[test]
    fn typst_tags_are_painted() {
        let ansi = render("#let x = 1\n= Heading\n", "typ", ThemeChoice::Dark);
        assert!(ansi.contains("\x1b[38;2;"));
        assert!(ansi.contains("Heading"));
    }

    #[test]
    fn typst_code_fence_uses_code_syntax() {
        let source = "let x = 1";
        assert!(highlight(source, "typ", ThemeChoice::Dark).runs.is_empty());
        assert!(!highlight(source, "typc", ThemeChoice::Dark).runs.is_empty());
    }

    #[test]
    fn just_styles_recipe_names_dependencies_and_interpolation() {
        let source = "mode := \"release\"\ncheck: prepare\n    cargo check --profile {{mode}}\n";
        let code = highlight(source, "justfile", ThemeChoice::Dark);
        let style_at = |index| {
            code.runs
                .iter()
                .find(|(range, _)| range.contains(&index))
                .map(|(_, style)| *style)
        };
        let command = style_at(source.find("cargo").unwrap());
        assert!(command.is_some());
        assert_eq!(command, style_at(source.find("--profile").unwrap()));
        for index in [
            source.find("release").unwrap(),
            source.find("check:").unwrap(),
            source.find("prepare").unwrap(),
            source.rfind("mode").unwrap(),
        ] {
            let style = style_at(index);
            assert!(style.is_some());
            assert_ne!(style, command);
        }
    }

    #[test]
    fn extra_fence_tags_select_their_syntax() {
        let cases = [
            ("lean4", "def twice (x : Nat) := x + x"),
            ("chez-scheme", "(define (twice x) (+ x x))"),
            ("r7rs", "(define-library (demo))"),
            ("fnl", "(fn twice [x] (+ x x))"),
            ("nu", "let x = 2"),
        ];
        for (tag, source) in cases {
            assert!(
                !highlight(source, tag, ThemeChoice::Dark).runs.is_empty(),
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
    fn builtin_aliases_keep_the_builtin_syntax() {
        let cases = [
            ("SBCL", "lisp", "(defun twice (x) (+ x x))"),
            ("ccl", "lisp", "(defun twice (x) (+ x x))"),
            ("babashka", "clj", "(defn twice [x] (+ x x))"),
            ("fsharp", "F#", "let x = 2"),
        ];
        for (alias, builtin, source) in cases {
            let expected = highlight(source, builtin, ThemeChoice::Dark);
            assert!(!expected.runs.is_empty(), "{builtin}");
            assert_eq!(
                highlight(source, alias, ThemeChoice::Dark).runs,
                expected.runs,
                "{alias}"
            );
        }
    }

    #[test]
    fn typst_raw_block_highlights_embedded_rust() {
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
