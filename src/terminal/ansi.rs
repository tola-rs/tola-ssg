//! Terminal text a command wrote: the styling it carries, and the control it must not have.
//!
//! A command writes text, carriage returns, and escape sequences to one stream. This module
//! interprets that stream wherever Tola draws it — live hook output, a development frame, or a
//! diagnostic note quoting a failed command — so one reading of a styling sequence decides what
//! a style means, and every other sequence is consumed instead of reaching the terminal.

use std::fmt::Write;

use anstyle_parse::{Params, ParamsIter, Perform};
use ratatui::style::{Color, Modifier, Style};

/// One piece of drawable text under the style in force where it appeared.
///
/// A segment is either visible characters without a line break, or the line break itself.
/// A reader that assembles lines splits on `text == "\n"` and never looks inside a style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) text: String,
    pub(crate) style: Style,
}

/// One output stream's terminal text, interpreted as it arrives.
///
/// The style in force and a deferred carriage return survive between calls, so a sequence or a
/// CRLF split across reads is interpreted as if it had arrived whole. The stream owns this value
/// for one command execution; a new execution starts from the default style.
pub(crate) struct Parser {
    parser: anstyle_parse::Parser,
    style: Style,
    carriage_return: bool,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self {
            parser: Default::default(),
            style: Style::default(),
            carriage_return: false,
        }
    }

    /// Interpret `chunk`, returning the segments it completes.
    pub(crate) fn push(&mut self, chunk: &str) -> Vec<Segment> {
        let mut segments = Vec::new();
        let mut writer = Writer {
            style: self.style,
            carriage_return: self.carriage_return,
            run: String::new(),
            segments: &mut segments,
        };
        for byte in chunk.bytes() {
            self.parser.advance(&mut writer, byte);
        }
        writer.finish();
        self.style = writer.style;
        self.carriage_return = writer.carriage_return;
        segments
    }

    /// Emit a deferred carriage return as the visible `\r` its line would show.
    ///
    /// A chunk that ends with a carriage return cannot tell whether a line feed follows, so the
    /// decision waits for the next chunk or for the line to close.
    pub(crate) fn flush(&mut self) -> Option<Segment> {
        if !self.carriage_return {
            return None;
        }
        self.carriage_return = false;
        Some(Segment {
            text: "\\r".to_owned(),
            style: self.style,
        })
    }
}

/// The whole `text`, interpreted in one call.
pub(crate) fn segments(text: &str) -> Vec<Segment> {
    let mut parser = Parser::new();
    let mut segments = parser.push(text);
    if let Some(carriage_return) = parser.flush() {
        segments.push(carriage_return);
    }
    segments
}

/// `text` as a terminal draws it: every control sequence consumed, the supported styling kept
/// when `styled`, and a reset closing a style the text left open.
pub(crate) fn paint(text: &str, styled: bool) -> String {
    let mut painted = String::with_capacity(text.len());
    let mut painter = Painter::new(styled);
    for segment in segments(text) {
        painter.write(&segment, &mut painted);
    }
    painter.reset(&mut painted);
    painted
}

/// Writes segments back as the escape sequences a terminal reads.
///
/// One painter tracks the style it last opened, so a style starts once where it changes and the
/// text it belongs to closes with a reset. A plain painter writes the text alone.
pub(crate) struct Painter {
    styled: bool,
    style: Style,
}

impl Painter {
    pub(crate) fn new(styled: bool) -> Self {
        Self {
            styled,
            style: Style::default(),
        }
    }

    pub(crate) fn write(&mut self, segment: &Segment, output: &mut String) {
        if !self.styled {
            output.push_str(&segment.text);
            return;
        }
        if segment.style != self.style {
            output.push_str(&sgr(segment.style));
            self.style = segment.style;
        }
        output.push_str(&segment.text);
    }

    pub(crate) fn reset(&mut self, output: &mut String) {
        if self.styled && self.style != Style::default() {
            output.push_str(&sgr(Style::default()));
        }
        self.style = Style::default();
    }
}

/// Spell a control character the way a reader can see it, so it occupies text instead of a
/// terminal function.
pub(crate) fn push_visible(output: &mut String, character: char) {
    match character {
        '\n' => output.push_str("\\n"),
        '\r' => output.push_str("\\r"),
        '\t' => output.push_str("\\t"),
        character if character.is_ascii_control() => {
            write!(output, "\\x{:02x}", character as u32).expect("writing to a String succeeds");
        }
        character if character.is_control() => {
            write!(output, "\\u{{{:x}}}", character as u32).expect("writing to a String succeeds");
        }
        character => output.push(character),
    }
}

/// Collects the text and styles one `push` produces.
struct Writer<'a> {
    style: Style,
    carriage_return: bool,
    run: String,
    segments: &'a mut Vec<Segment>,
}

impl Writer<'_> {
    fn finish(&mut self) {
        self.flush_run();
    }

    /// Text before a newline is a segment of its own, so a line is never guessed from a style.
    fn newline(&mut self) {
        self.flush_run();
        self.segments.push(Segment {
            text: "\n".to_owned(),
            style: self.style,
        });
    }

    fn flush_run(&mut self) {
        if self.run.is_empty() {
            return;
        }
        self.segments.push(Segment {
            text: std::mem::take(&mut self.run),
            style: self.style,
        });
    }

    fn begin(&mut self) {
        if std::mem::take(&mut self.carriage_return) {
            self.run.push_str("\\r");
        }
    }
}

impl Perform for Writer<'_> {
    fn print(&mut self, character: char) {
        self.begin();
        if character.is_control() {
            push_visible(&mut self.run, character);
        } else {
            self.run.push(character);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => {
                if self.carriage_return {
                    self.run.push_str("\\r");
                }
                self.carriage_return = true;
            }
            b'\n' => {
                self.carriage_return = false;
                self.newline();
            }
            b'\t' => {
                self.begin();
                self.run.push('\t');
            }
            byte => {
                self.begin();
                push_visible(&mut self.run, char::from(byte));
            }
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], ignore: bool, action: u8) {
        if action == b'm' {
            self.flush_run();
            apply_sgr(&mut self.style, params, ignore);
        }
    }
}

/// Apply one SGR sequence; `ignore` marks a parameter list the parser could not hold whole.
fn apply_sgr(style: &mut Style, params: &Params, ignore: bool) {
    if ignore {
        return;
    }
    let mut params = params.iter();
    while let Some(param) = params.next() {
        apply_param(style, param, &mut params);
    }
}

fn apply_param(style: &mut Style, param: &[u16], params: &mut ParamsIter<'_>) {
    match param[0] {
        0 => *style = Style::default(),
        1 => *style = style.add_modifier(Modifier::BOLD),
        2 => *style = style.add_modifier(Modifier::DIM),
        4 => *style = style.add_modifier(Modifier::UNDERLINED),
        22 => style.add_modifier.remove(Modifier::BOLD | Modifier::DIM),
        24 => style.add_modifier.remove(Modifier::UNDERLINED),
        30..=37 | 90..=97 => *style = style.fg(basic_color(param[0])),
        39 => style.fg = None,
        40..=47 | 100..=107 => *style = style.bg(basic_color(param[0] - 10)),
        49 => style.bg = None,
        38 | 48 | 58 => apply_color(style, param[0], &param[1..], params),
        _ => {}
    }
}

/// Apply a colour parameter. A semicolon form spreads its components over the following
/// parameters, and they are consumed even when the colour is unsupported, so a component can
/// never be read as a styling code of its own.
fn apply_color(style: &mut Style, code: u16, subparams: &[u16], params: &mut ParamsIter<'_>) {
    let color = if subparams.is_empty() {
        semicolon_color(params)
    } else {
        subparam_color(subparams)
    };
    let Some(color) = color else {
        return;
    };
    match code {
        38 => style.fg = Some(color),
        48 => style.bg = Some(color),
        _ => {}
    }
}

fn semicolon_color(params: &mut ParamsIter<'_>) -> Option<Color> {
    match params.next()? {
        [5] => params.next().and_then(first).and_then(indexed),
        [5, index] => indexed(*index),
        [2] => {
            let red = params.next().and_then(first);
            let green = params.next().and_then(first);
            let blue = params.next().and_then(first);
            rgb(red?, green?, blue?)
        }
        [2, red, green, blue] => rgb(*red, *green, *blue),
        _ => None,
    }
}

fn subparam_color(subparams: &[u16]) -> Option<Color> {
    match subparams {
        [5, index] => indexed(*index),
        [2, red, green, blue] => rgb(*red, *green, *blue),
        [2, 0, red, green, blue] => rgb(*red, *green, *blue),
        _ => None,
    }
}

fn first(param: &[u16]) -> Option<u16> {
    param.first().copied()
}

fn indexed(value: u16) -> Option<Color> {
    u8::try_from(value).ok().map(Color::Indexed)
}

fn rgb(red: u16, green: u16, blue: u16) -> Option<Color> {
    Some(Color::Rgb(
        u8::try_from(red).ok()?,
        u8::try_from(green).ok()?,
        u8::try_from(blue).ok()?,
    ))
}

fn basic_color(value: u16) -> Color {
    const COLORS: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    COLORS[usize::from(if value >= 90 {
        value - 90 + 8
    } else {
        value - 30
    })]
}

fn sgr(style: Style) -> String {
    let mut escape = String::from("\x1b[0");
    if style.add_modifier.contains(Modifier::BOLD) {
        escape.push_str(";1");
    }
    if style.add_modifier.contains(Modifier::DIM) {
        escape.push_str(";2");
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        escape.push_str(";4");
    }
    if let Some(color) = style.fg {
        push_color(&mut escape, color, false);
    }
    if let Some(color) = style.bg {
        push_color(&mut escape, color, true);
    }
    escape.push('m');
    escape
}

fn push_color(escape: &mut String, color: Color, background: bool) {
    let (basic, bright, extended) = if background {
        (40, 100, 48)
    } else {
        (30, 90, 38)
    };
    let index = match color {
        Color::Reset => return,
        Color::Indexed(index) => {
            write!(escape, ";{extended};5;{index}").expect("writing to a String succeeds");
            return;
        }
        Color::Rgb(red, green, blue) => {
            write!(escape, ";{extended};2;{red};{green};{blue}")
                .expect("writing to a String succeeds");
            return;
        }
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    let value = if index < 8 {
        basic + index
    } else {
        bright + index - 8
    };
    write!(escape, ";{value}").expect("writing to a String succeeds");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_of(source: &str, text: &str) -> Style {
        segments(source)
            .into_iter()
            .find(|segment| segment.text == text)
            .unwrap_or_else(|| panic!("{text} is not drawn"))
            .style
    }

    fn painted(segments: &[Segment], styled: bool) -> String {
        let mut output = String::new();
        let mut painter = Painter::new(styled);
        for segment in segments {
            painter.write(segment, &mut output);
        }
        painter.reset(&mut output);
        output
    }

    #[test]
    fn sgr_sets_colors_and_emphasis() {
        let default = Style::default();
        let cases = [
            ("\x1b[31mred", "red", default.fg(Color::Red)),
            ("\x1b[91mbright", "bright", default.fg(Color::LightRed)),
            ("\x1b[42mback", "back", default.bg(Color::Green)),
            (
                "\x1b[38;5;196mindex",
                "index",
                default.fg(Color::Indexed(196)),
            ),
            (
                "\x1b[48;2;1;2;3mrgb",
                "rgb",
                default.bg(Color::Rgb(1, 2, 3)),
            ),
            (
                "\x1b[38:5:200mcolon",
                "colon",
                default.fg(Color::Indexed(200)),
            ),
            (
                "\x1b[38:2:9:8:7mplain",
                "plain",
                default.fg(Color::Rgb(9, 8, 7)),
            ),
            (
                "\x1b[38:2::9:8:7mspace",
                "space",
                default.fg(Color::Rgb(9, 8, 7)),
            ),
            (
                "\x1b[1;2;4mfancy",
                "fancy",
                default.add_modifier(Modifier::BOLD | Modifier::DIM | Modifier::UNDERLINED),
            ),
            ("\x1b[1;31m\x1b[22;39mclear", "clear", default),
            ("\x1b[31;42m\x1b[49mhalf", "half", default.fg(Color::Red)),
            ("\x1b[mreset", "reset", default),
        ];
        for (source, text, expected) in cases {
            assert_eq!(style_of(source, text), expected, "{source:?}");
        }
    }

    #[test]
    fn unsupported_sgr_leaves_following_text_plain() {
        for source in [
            "\x1b[3;5;7;8;9mplain",
            "\x1b[58;2;9;8;7mplain",
            "\x1b[38;2;999;0;0mplain",
            "\x1b[38:2:1:9:8:7mplain",
        ] {
            assert_eq!(style_of(source, "plain"), Style::default(), "{source:?}");
        }
    }

    #[test]
    fn paint_emits_normalized_sgr() {
        assert_eq!(paint("\x1b[31mred\x1b[0m", true), "\x1b[0;31mred\x1b[0m");
        assert_eq!(paint("\x1b[31mred\x1b[0m", false), "red");
        assert_eq!(
            paint("\x1b[1;38;5;200mx", true),
            "\x1b[0;1;38;5;200mx\x1b[0m"
        );
    }

    #[test]
    fn chunk_boundaries_do_not_change_text() {
        let source = "a\x1b[31mred\x1b[0m\r\nb\x1b]8;;u\x1b\\link\x1b]8;;\x1b\\\tend";
        let whole = segments(source);
        for split in 0..=source.len() {
            if !source.is_char_boundary(split) {
                continue;
            }
            let mut parser = Parser::new();
            let mut parts = parser.push(&source[..split]);
            parts.extend(parser.push(&source[split..]));
            if let Some(carriage_return) = parser.flush() {
                parts.push(carriage_return);
            }
            for styled in [false, true] {
                assert_eq!(
                    painted(&parts, styled),
                    painted(&whole, styled),
                    "split at {split}, styled {styled}"
                );
            }
        }
    }

    #[test]
    fn control_sequences_never_reach_text() {
        let source = "a\x1b[2Jb\x1b[1;2Hc\x1b]0;title\x07d\x1b]8;;http://x\x1b\\e\x1b]8;;\x1b\\f\x1bPq\x1b\\g";
        assert_eq!(paint(source, false), "abcdefg");
        assert_eq!(paint(source, true), "abcdefg");
    }

    #[test]
    fn carriage_returns_stay_visible() {
        assert_eq!(paint("a\r\nb", false), "a\nb");
        assert_eq!(paint("a\rb", false), "a\\rb");
        assert_eq!(paint("a\r", false), "a\\r");
        assert_eq!(paint("a\r\rb", false), "a\\r\\rb");
    }

    #[test]
    fn style_survives_newlines() {
        let segments = segments("\x1b[31mfirst\nsecond");
        assert!(
            segments
                .iter()
                .filter(|segment| segment.text != "\n")
                .all(|segment| segment.style.fg == Some(Color::Red))
        );
    }

    #[test]
    fn newlines_stand_alone() {
        for segment in segments("a\nb\n\nc\x1b[31m\n") {
            assert!(
                segment.text == "\n" || !segment.text.contains('\n'),
                "{:?}",
                segment.text
            );
        }
    }
}
