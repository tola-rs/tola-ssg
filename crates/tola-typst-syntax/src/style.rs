//! The writing style a formatter is asked for.

use serde::{Deserialize, Serialize};

/// How a caller wants a Typst source formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Spaces per level of indentation.
    pub tab_spaces: usize,
    /// The column the formatter wraps at.
    pub max_width: usize,
    /// How markup prose is wrapped.
    pub prose_wrap: ProseWrap,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tab_spaces: 2,
            max_width: 120,
            prose_wrap: ProseWrap::None,
        }
    }
}

/// How markup prose is wrapped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProseWrap {
    /// Keep the author's own line breaks.
    #[default]
    None,
    /// Wrap prose to fill each line.
    Fill,
    /// Place each sentence on its own line.
    Sentence,
}
