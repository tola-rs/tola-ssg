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
    /// Enable feed generation.
    pub enable: bool,
    /// Output path for feed file.
    #[config(default = "feed.xml")]
    pub path: PathBuf,
    /// Feed format (RSS 2.0 or Atom 1.0).
    #[config(default = "rss")]
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
        assert!(!config.build.feed.enable);
        assert_eq!(config.build.feed.path, PathBuf::from("feed.xml"));
        assert_eq!(config.build.feed.format, FeedFormat::Rss);
    }

    #[test]
    fn test_custom_config() {
        let config =
            test_parse_config("[build.feed]\nenable = true\npath = \"rss.xml\"\nformat = \"atom\"");
        assert!(config.build.feed.enable);
        assert_eq!(config.build.feed.path, PathBuf::from("rss.xml"));
        assert_eq!(config.build.feed.format, FeedFormat::Atom);
    }
}
