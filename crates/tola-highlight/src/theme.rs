use std::sync::LazyLock;

use syntect::highlighting::{Color, FontStyle, Theme, ThemeItem, ThemeSettings};

/// Palette used to paint a terminal preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Terminal's ANSI colors and default foreground.
    Terminal,
    /// Dark palette for terminal previews.
    Dark,
    /// Light palette for terminal previews.
    Light,
    /// No ANSI escapes; source is returned sanitized and tab-expanded.
    Plain,
}

impl ThemeChoice {
    /// Parse a theme name, returning `None` if unknown.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "terminal" => Some(Self::Terminal),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "plain" | "none" | "never" => Some(Self::Plain),
            _ => None,
        }
    }

    /// Canonical name of this theme choice.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Plain => "plain",
        }
    }
}

static TERMINAL: LazyLock<Theme> = LazyLock::new(|| palette(ThemeChoice::Terminal));
static DARK: LazyLock<Theme> = LazyLock::new(|| palette(ThemeChoice::Dark));
static LIGHT: LazyLock<Theme> = LazyLock::new(|| palette(ThemeChoice::Light));

pub(crate) fn theme_of(choice: ThemeChoice) -> Option<&'static Theme> {
    match choice {
        ThemeChoice::Terminal => Some(&TERMINAL),
        ThemeChoice::Dark => Some(&DARK),
        ThemeChoice::Light => Some(&LIGHT),
        ThemeChoice::Plain => None,
    }
}

fn color(rgb: u32) -> Option<Color> {
    Some(Color {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: 0xff,
    })
}

fn item(scope: &str, fg: Option<u32>, font_style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: scope.parse().expect("theme scope parses"),
        style: syntect::highlighting::StyleModifier {
            foreground: fg.and_then(color),
            background: None,
            font_style,
        },
    }
}

fn palette(choice: ThemeChoice) -> Theme {
    let (
        fg,
        comment,
        escape,
        delim,
        heading,
        marker,
        label,
        keyword,
        constant,
        string,
        func,
        macro_,
        annot,
        other,
        diff,
        inserted,
        deleted,
        key,
        val,
        invalid,
    ) = match choice {
        ThemeChoice::Terminal => (
            39, 90, 36, 36, 39, 35, 36, 35, 33, 32, 36, 36, 33, 35, 35, 32, 31, 36, 32, 31,
        ),
        ThemeChoice::Dark => (
            0xc9d1d9, 0x8b949e, 0x56d4dd, 0x56d4dd, 0xe6edf3, 0xd2a8ff, 0x79c0ff, 0xff7b72,
            0xffa657, 0xa5d6a7, 0x79c0ff, 0x79c0ff, 0xffa657, 0xd2a8ff, 0xd2a8ff, 0x7ee787,
            0xff7b72, 0x79c0ff, 0xa5d6a7, 0xff7b72,
        ),
        ThemeChoice::Light => (
            0x24292f, 0x74747c, 0x1d6c76, 0x1d6c76, 0x1f2328, 0x8b41b1, 0x1d6c76, 0xd73948,
            0xb60157, 0x198810, 0x4b69c6, 0x16718d, 0x301414, 0x8b41b1, 0x8b41b1, 0x198810,
            0xd73948, 0x4b69c6, 0x198810, 0xd73948,
        ),
        ThemeChoice::Plain => unreachable!("plain text has no theme"),
    };
    let bold = Some(FontStyle::BOLD);
    let italic = Some(FontStyle::ITALIC);
    let underline = Some(FontStyle::UNDERLINE);
    Theme {
        name: Some(choice.name().into()),
        author: Some("The Tola Project Developers".into()),
        settings: ThemeSettings {
            foreground: color(fg),
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
            item("punctuation.definition.math", Some(delim), None),
            item(
                "keyword.operator.math, punctuation.math.typst",
                Some(delim),
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
            item("entity.name, variable.function, support", Some(func), None),
            item("support.macro", Some(macro_), None),
            item("meta.annotation", Some(annot), None),
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
                Some(val),
                None,
            ),
            item("invalid, invalid.typst", Some(invalid), underline),
        ],
    }
}
