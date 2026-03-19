//! URL slug configuration.

use serde::{Deserialize, Serialize};

/// URL slug generation mode for paths and anchors
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlugMode {
    /// Full slugify: Unicode -> ASCII, lowercase, use separator.
    Full,
    /// Safe mode: remove dangerous chars, preserve Unicode, use separator (default).
    #[default]
    Safe,
    /// ASCII mode: transliterate Unicode -> ASCII, use separator.
    Ascii,
    /// No modification; preserve original text.
    No,
}

/// Case transformation mode for slugs
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlugCase {
    /// Convert to lowercase (default).
    #[default]
    Lower,
    /// Convert to UPPERCASE.
    Upper,
    /// Capitalize each word (Title Case).
    Capitalize,
    /// Preserve original case.
    Preserve,
}

/// Separator character for slugs
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlugSeparator {
    /// Dash separator (`-`) (default).
    #[default]
    Dash,
    /// Underscore separator (`_`).
    Underscore,
}

impl SlugSeparator {
    /// Get the character representation.
    pub const fn as_char(&self) -> char {
        match self {
            Self::Dash => '-',
            Self::Underscore => '_',
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlugConfig {
    /// Slugify URL paths.
    pub path: SlugMode,
    /// Slugify URL fragments (anchors).
    pub fragment: SlugMode,
    /// Separator character for spaces.
    pub separator: SlugSeparator,
    /// Case transformation.
    pub case: SlugCase,
}

impl Default for SlugConfig {
    fn default() -> Self {
        Self {
            path: SlugMode::Safe,
            fragment: SlugMode::Full,
            separator: SlugSeparator::Dash,
            case: SlugCase::Lower,
        }
    }
}
