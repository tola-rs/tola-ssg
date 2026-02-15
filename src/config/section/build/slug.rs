//! URL slug configuration.

use serde::{Deserialize, Serialize};

/// URL slug generation mode for paths and anchors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlugMode {
    /// Full slugify: Unicode → ASCII, lowercase, use separator.
    Full,
    /// Safe mode: remove dangerous chars, preserve Unicode, use separator (default).
    #[default]
    Safe,
    /// ASCII mode: transliterate Unicode → ASCII, use separator.
    Ascii,
    /// No modification; preserve original text.
    No,
}

/// Case transformation mode for slugs.
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

/// Separator character for slugs.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_defaults() {
        let config = test_parse_config("");
        assert_eq!(config.build.slug.path, SlugMode::Safe);
        assert_eq!(config.build.slug.fragment, SlugMode::Full);
        assert_eq!(config.build.slug.separator, SlugSeparator::Dash);
        assert_eq!(config.build.slug.case, SlugCase::Lower);
    }

    #[test]
    fn test_mode_parsing() {
        for (input, expected) in [
            ("full", SlugMode::Full),
            ("safe", SlugMode::Safe),
            ("ascii", SlugMode::Ascii),
            ("no", SlugMode::No),
        ] {
            let config = test_parse_config(&format!(
                "[build.slug]\npath = \"{input}\"\nfragment = \"{input}\""
            ));
            assert_eq!(config.build.slug.path, expected, "path failed for {input}");
            assert_eq!(
                config.build.slug.fragment, expected,
                "fragment failed for {input}"
            );
        }
    }

    #[test]
    fn test_separator_parsing() {
        let config = test_parse_config("[build.slug]\nseparator = \"underscore\"");
        assert_eq!(config.build.slug.separator, SlugSeparator::Underscore);
        assert_eq!(config.build.slug.separator.as_char(), '_');

        let config = test_parse_config("[build.slug]\nseparator = \"dash\"");
        assert_eq!(config.build.slug.separator.as_char(), '-');
    }

    #[test]
    fn test_case_parsing() {
        for (input, expected) in [
            ("lower", SlugCase::Lower),
            ("upper", SlugCase::Upper),
            ("capitalize", SlugCase::Capitalize),
            ("preserve", SlugCase::Preserve),
        ] {
            let config = test_parse_config(&format!("[build.slug]\ncase = \"{input}\""));
            assert_eq!(config.build.slug.case, expected, "case failed for {input}");
        }
    }
}
