//! Syntax highlighting shared by Tola's terminal documentation and Nu previews.
//!
//! Non-Typst languages use the same `syntect` syntax set (`two_face`) that Typst
//! embeds for raw blocks, so a preview matches the published HTML. Typst sources
//! use the compiler's own tags through [`typst_syntax::Tag::tm_scope`].

use std::sync::LazyLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, Highlighter, Style, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{Scope, SyntaxSet};
use typst_syntax::{LinkedNode, SyntaxKind, Tag};

/// The palette used to paint a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    /// A dark palette for the default TUI surface.
    Dark,
    /// The light palette Typst uses for compiled HTML.
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

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);
static DARK: LazyLock<Theme> = LazyLock::new(|| palette(true));
static LIGHT: LazyLock<Theme> = LazyLock::new(|| palette(false));

/// The syntax definitions shared with Typst's raw blocks.
pub fn syntaxes() -> &'static SyntaxSet {
    &SYNTAXES
}

/// Highlight `source` as `language` and return ANSI text.
///
/// `language` is a fence tag or a file extension (`rust`, `rs`, `toml`, `typ`).
/// Unknown languages and [`ThemeChoice::Plain`] return the sanitized source
/// unchanged.
pub fn render(source: &str, language: &str, theme: ThemeChoice) -> String {
    let source = sanitize(source);
    let Some(theme) = theme_of(theme) else {
        return source;
    };
    if is_typst(language) {
        render_typst(&source, theme)
    } else {
        render_syntect(&source, language, theme)
    }
}

fn theme_of(choice: ThemeChoice) -> Option<&'static Theme> {
    match choice {
        ThemeChoice::Dark => Some(&DARK),
        ThemeChoice::Light => Some(&LIGHT),
        ThemeChoice::Plain => None,
    }
}

fn is_typst(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "typ" | "typst" | "typc" | "typm" | "typst-code" | "typst-math"
    )
}

fn render_syntect(source: &str, language: &str, theme: &Theme) -> String {
    let token = language.trim().to_ascii_lowercase();
    let Some(syntax) = SYNTAXES
        .find_syntax_by_token(&token)
        .or_else(|| SYNTAXES.find_syntax_by_extension(&token))
    else {
        return source.to_owned();
    };
    let mut lines = HighlightLines::new(syntax, theme);
    let mut out = String::with_capacity(source.len() + source.len() / 4);
    for line in source.split_inclusive('\n') {
        let (text, newline) = match line.strip_suffix('\n') {
            Some(text) => (text, "\n"),
            None => (line, ""),
        };
        let text = text.strip_suffix('\r').unwrap_or(text);
        match lines.highlight_line(text, &SYNTAXES) {
            Ok(spans) => {
                for (style, piece) in spans {
                    push_ansi(&mut out, style, piece);
                }
            }
            Err(_) => out.push_str(text),
        }
        out.push_str(newline);
    }
    out
}

fn render_typst(source: &str, theme: &Theme) -> String {
    let root = typst_syntax::parse(source);
    let highlighter = Highlighter::new(theme);
    let mut out = String::with_capacity(source.len() + source.len() / 4);
    let mut offset = 0;
    walk_typst(
        &LinkedNode::new(&root),
        None,
        source,
        &mut offset,
        &highlighter,
        theme,
        &mut out,
    );
    out
}

fn walk_typst(
    node: &LinkedNode<'_>,
    inherited: Option<Style>,
    source: &str,
    offset: &mut usize,
    highlighter: &Highlighter<'_>,
    theme: &Theme,
    out: &mut String,
) {
    if node.kind() == SyntaxKind::Raw
        && let Some(raw) = node.cast::<typst_syntax::ast::Raw>()
        && let Some(lang) = raw.lang()
    {
        let ranges: Vec<_> = node
            .children()
            .filter(|child| child.kind() == SyntaxKind::Text)
            .map(|child| child.range())
            .collect();
        if let (Some(first), Some(last)) = (ranges.first(), ranges.last()) {
            let range = node.range();
            let prefix = &source[range.start..first.start];
            let body = &source[first.start..last.end];
            let suffix = &source[last.end..range.end];
            let raw_style = scope_style(highlighter, Tag::Raw);
            if let Some(style) = raw_style {
                push_ansi(out, style, prefix);
            } else {
                out.push_str(prefix);
            }
            let language = lang.get().trim();
            if is_typst(language) {
                out.push_str(&render_typst(body, theme));
            } else {
                out.push_str(&render_syntect(body, language, theme));
            }
            if let Some(style) = raw_style {
                push_ansi(out, style, suffix);
            } else {
                out.push_str(suffix);
            }
            *offset = range.end;
            return;
        }
    }
    let tag = typst_syntax::highlight(node);
    let style = tag
        .and_then(|tag| scope_style(highlighter, tag))
        .or(inherited);
    let leaf = node.leaf_text();
    if !leaf.is_empty() {
        let start = *offset;
        *offset += leaf.len();
        debug_assert_eq!(&source[start..*offset], leaf);
        match style {
            Some(style) => push_ansi(out, style, leaf),
            None => out.push_str(leaf),
        }
        return;
    }
    for child in node.children() {
        walk_typst(&child, style, source, offset, highlighter, theme, out);
    }
}

fn scope_style(highlighter: &Highlighter<'_>, tag: Tag) -> Option<Style> {
    let scope = Scope::new(tag.tm_scope()).ok()?;
    Some(highlighter.style_for_stack(&[scope]))
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
