//! Feed (RSS/Atom) generation configuration.

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Feed output format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeedFormat {
    /// RSS 2.0 format (default).
    #[default]
    Rss,
    /// Atom 1.0 format.
    Atom,
}

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.feed")]
pub struct FeedConfig {
    #[config(inline_doc = "Enable feed generation.")]
    pub enable: bool,
    #[config(default = "feed.xml", inline_doc = "Output path for feed file.")]
    pub path: PathBuf,
    #[config(default = "rss", inline_doc = "Feed format: rss | atom.")]
    pub format: FeedFormat,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            enable: false,
            path: "feed.xml".into(),
            format: FeedFormat::Rss,
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
        assert!(!config.site.feed.enable);
        assert_eq!(config.site.feed.path, PathBuf::from("feed.xml"));
        assert_eq!(config.site.feed.format, FeedFormat::Rss);
    }

    #[test]
    fn test_custom_config() {
        let config =
            test_parse_config("[site.feed]\nenable = true\npath = \"rss.xml\"\nformat = \"atom\"");
        assert!(config.site.feed.enable);
        assert_eq!(config.site.feed.path, PathBuf::from("rss.xml"));
        assert_eq!(config.site.feed.format, FeedFormat::Atom);
    }
}
