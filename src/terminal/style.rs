//! The styling roles terminal status text is drawn with.

use owo_colors::OwoColorize;
use ratatui::style::{Modifier, Style};
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

    /// The style of secondary text and key hints.
    pub(crate) fn dim_style(self) -> Style {
        if self.uses_color {
            Style::default().add_modifier(Modifier::DIM)
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
