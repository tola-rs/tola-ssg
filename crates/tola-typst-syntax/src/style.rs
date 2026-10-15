//! The writing style a formatter is asked for.

use serde::{Deserialize, Serialize};

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
