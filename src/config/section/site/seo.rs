//! SEO configuration (feed, sitemap, OG tags).

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Feed output format
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
#[config(section = "site.seo.feed")]
pub struct FeedConfig {
    #[config(inline_doc = "Enable feed generation")]
    pub enable: bool,
    #[config(default = "feed.xml", inline_doc = "Output path for feed file")]
    pub path: PathBuf,
    #[config(default = "rss", inline_doc = "Feed format: rss | atom")]
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

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.seo.sitemap")]
pub struct SitemapConfig {
    #[config(inline_doc = "Enable sitemap generation")]
    pub enable: bool,
    #[config(inline_doc = "Output path for sitemap file")]
    pub path: PathBuf,
}

impl Default for SitemapConfig {
    fn default() -> Self {
        Self {
            enable: false,
            path: "sitemap.xml".into(),
        }
    }
}

/// SEO configuration containing feed, sitemap, and OG tag settings
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.seo")]
pub struct SeoConfig {
    #[config(inline_doc = "Auto-inject OG meta tags (can be overridden in Typst)")]
    pub auto_og: bool,

    /// Feed generation settings (RSS/Atom)
    #[config(sub)]
    pub feed: FeedConfig,

    /// Sitemap generation settings
    #[config(sub)]
    pub sitemap: SitemapConfig,
}
