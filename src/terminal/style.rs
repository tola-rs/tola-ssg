//! The styling roles terminal status text is drawn with.

use owo_colors::OwoColorize;
use ratatui::style::{Color, Modifier, Style};
use tracing::Level;

/// Every role of terminal status text, under one color decision.
///
/// The palette is built once from the single use-color decision at the terminal boundary and
/// handed to each producer, so no role re-decides whether escape sequences are written. A role
/// styling part of a line takes only that part: [`Palette::summary`] styles the first word of the
/// line it is given, [`Palette::serving`] the address, [`Palette::hook_result`] the outcome word.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    uses_color: bool,
}

impl Palette {
    pub(crate) const fn new(uses_color: bool) -> Self {
        Self { uses_color }
    }

    /// Whether this palette writes escape sequences.
    pub(crate) const fn uses_color(self) -> bool {
        self.uses_color
    }

    /// The first word of a status summary, which is what the reader scans for.
    pub(crate) fn summary(self, text: &str) -> String {
        if !self.uses_color {
            return text.to_owned();
        }
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        format!(
            "{}{}",
            self.with_style(&text[..end], |word| word.green().bold().to_string()),
            &text[end..]
        )
    }

    /// The status line of a round that published nothing.
    pub(crate) fn failure(self, text: &str) -> String {
        self.with_style(text, |text| text.red().bold().to_string())
    }

    /// A notice naming what the author has to change.
    pub(crate) fn notice(self, text: &str) -> String {
        self.with_style(text, |text| text.yellow().to_string())
    }

    /// A transient or secondary status line.
    pub(crate) fn secondary(self, text: &str) -> String {
        self.with_style(text, |text| text.dimmed().to_string())
    }

    /// The label every line of a running hook's output carries.
    pub(crate) fn stream_label(self, text: &str) -> String {
        self.with_style(text, |text| text.dimmed().to_string())
    }

    /// The address the development server serves, so only the part the reader opens is emphasized.
    pub(crate) fn serving(self, url: &str) -> String {
        self.with_style(url, |url| url.cyan().underline().to_string())
    }

    /// The accent a hook name carries wherever one is named.
    pub(crate) fn hook_name(self, name: &str) -> String {
        self.with_style(name, |name| name.cyan().to_string())
    }

    /// The accent a stage carries, distinct from the names it owns.
    pub(crate) fn hook_stage(self, stage: &str) -> String {
        self.with_style(stage, |stage| stage.magenta().to_string())
    }

    /// A section header of a report the reader scans.
    pub(crate) fn heading(self, text: &str) -> String {
        self.with_style(text, |text| text.bold().to_string())
    }

    /// The outcome word of a finished hook, bold green when it worked and bold red when it failed.
    pub(crate) fn hook_result(self, success: bool, text: &str) -> String {
        if success {
            self.with_style(text, |text| text.green().bold().to_string())
        } else {
            self.with_style(text, |text| text.red().bold().to_string())
        }
    }

    /// A tracing level's lowercase word, drawn with the level's own emphasis.
    pub(crate) fn level_word(self, level: Level) -> String {
        let word = level.as_str().to_ascii_lowercase();
        match level {
            Level::ERROR => self.with_style(&word, |word| word.red().bold().to_string()),
            Level::WARN => self.with_style(&word, |word| word.yellow().bold().to_string()),
            Level::INFO => self.with_style(&word, |word| word.green().bold().to_string()),
            Level::DEBUG => self.with_style(&word, |word| word.cyan().bold().to_string()),
            Level::TRACE => self.with_style(&word, |word| word.dimmed().to_string()),
        }
    }

    /// The style a heading of `level` draws with, matching the plain renderer's colors.
    ///
    /// A heading is an anchor a reader can jump to, so it is underlined like every other
    /// followable target, through the same color gate.
    pub(crate) fn heading_style(self, level: u8) -> Style {
        self.view_style(
            match level {
                1 => Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
                2 => Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                3 => Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                _ => Style::default().fg(Color::Green),
            }
            .add_modifier(Modifier::UNDERLINED),
        )
    }

    /// The style of emphasized text.
    pub(crate) fn emphasis_style(self) -> Style {
        self.view_style(Style::default().add_modifier(Modifier::BOLD))
    }
    /// The style of secondary text: borders, key hints, inactive tabs, and other reading aids.
    pub(crate) fn dim_style(self) -> Style {
        self.view_style(Style::default().add_modifier(Modifier::DIM))
    }

    /// The style of a name, label, or code span the reader scans for.
    pub(crate) fn accent_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Cyan))
    }

    /// The style of the row, line, or tab the reader has selected.
    pub(crate) fn selected_style(self) -> Style {
        self.view_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
    }

    /// The style of the cells one mouse drag selected.
    pub(crate) fn selection_style(self) -> Style {
        self.view_style(Style::default().add_modifier(Modifier::REVERSED))
    }

    /// The style of the search hit the reader is on.
    pub(crate) fn hit_style(self) -> Style {
        self.view_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED),
        )
    }

    /// The style of a code keyword, a boolean, or an operator.
    pub(crate) fn keyword_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Magenta))
    }

    /// The style of a code string.
    pub(crate) fn string_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Green))
    }

    /// The style of a code number.
    pub(crate) fn number_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Yellow))
    }

    /// The style of a code comment.
    pub(crate) fn comment_style(self) -> Style {
        self.view_style(Style::default().add_modifier(Modifier::DIM))
    }

    /// The style of a code literal: a name, a key, or a label.
    pub(crate) fn literal_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Cyan))
    }

    /// The style of a followable target: a cross-reference's label, or a heading's anchor.
    pub(crate) fn link_style(self) -> Style {
        self.view_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        )
    }

    /// The style of a followable target the pointer rests on, emphasized so it reads clickable.
    ///
    /// The pointer's own highlight, distinct from the static link: bold and underlined in the
    /// link's color, never reversed, so it cannot be mistaken for the search line or a jumped label.
    pub(crate) fn link_hover_style(self) -> Style {
        self.view_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    }

    /// The style of the status line, and of a search hit the reader is not on.
    pub(crate) fn notice_style(self) -> Style {
        self.view_style(Style::default().fg(Color::Yellow))
    }

    /// The style of a jump-mode label drawn beside the target it names.
    pub(crate) fn label_style(self) -> Style {
        self.view_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    }

    /// `style` when this palette writes escape sequences, and no styling otherwise.
    fn view_style(self, style: Style) -> Style {
        if self.uses_color {
            style
        } else {
            Style::default()
        }
    }

    /// `text` under `style`, or as written when this palette writes no escape sequences.
    fn with_style(self, text: &str, style: impl FnOnce(&str) -> String) -> String {
        if self.uses_color {
            style(text)
        } else {
            text.to_owned()
        }
    }
}
