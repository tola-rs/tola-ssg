//! `[site.info]` configuration (formerly `[site]`).
//!
//! Contains basic site information like title, author, description, etc.
//! These values are accessible in Typst via `@tola/site` virtual package.

use crate::config::section::FeedConfig;
use macros::Config;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// Site metadata for feed generation and Typst templates
/// Access in Typst via `#import "@tola/site:0.0.0": info`
/// For custom fields, use `[site.info.extra]` and access via `info.extra.xxx`
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.info")]
pub struct SiteInfoConfig {
    #[config(inline_doc = "Site title")]
    pub title: String,

    #[config(inline_doc = "Author name")]
    pub author: String,

    #[config(inline_doc = "Author email")]
    pub email: String,

    #[config(inline_doc = "Site description")]
    pub description: String,

    #[config(inline_doc = "Site URL, path used as prefix (e.g., \"https://example.com/blog/docs\")")]
    pub url: Option<String>,

    #[config(default = "en", inline_doc = "Language code (e.g., \"en\", \"zh-Hans\")")]
    pub language: String,

    #[config(inline_doc = "Copyright notice")]
    pub copyright: String,

    /// Custom fields accessible via `sys.inputs.extra.xxx` in Typst
    #[serde(default)]
    #[config(skip)]
    pub extra: FxHashMap<String, toml::Value>,
}

impl Default for SiteInfoConfig {
    fn default() -> Self {
        Self {
            title: String::new(),
            author: String::new(),
            email: String::new(),
            description: String::new(),
            url: None,
            language: "en".into(),
            copyright: String::new(),
            extra: FxHashMap::default(),
        }
    }
}

impl SiteInfoConfig {
    /// Validate site configuration.
    ///
    /// # Checks
    /// - If `feed_enabled`, `url` must be set
    /// - `url` must be a valid URL with scheme (e.g., `https://example.com`)
    pub fn validate(&self, feed_enabled: bool, diag: &mut crate::config::ConfigDiagnostics) {
        // Feed requires url
        if feed_enabled && self.url.is_none() {
            diag.error_with_hint(
                Self::FIELDS.url,
                format!(
                    "{} is enabled but {} is not configured",
                    FeedConfig::FIELDS.enable,
                    Self::FIELDS.url
                ),
                format!("set {}, e.g.: \"https://example.com\"", Self::FIELDS.url),
            );
        }

        // URL format check using url crate for strict validation
        if let Some(url_str) = &self.url {
            match url::Url::parse(url_str) {
                Ok(parsed) => {
                    // Must be http or https
                    if !matches!(parsed.scheme(), "http" | "https") {
                        diag.error_with_hint(
                            Self::FIELDS.url,
                            format!(
                                "scheme '{}' not supported, must be http or https",
                                parsed.scheme()
                            ),
                            "use format like https://example.com",
                        );
                    }
                    // Must have a valid host
                    if parsed.host_str().is_none() {
                        diag.error_with_hint(
                            Self::FIELDS.url,
                            "URL must have a valid host",
                            "use format like https://example.com",
                        );
                    }
                }
                Err(e) => {
                    diag.error_with_hint(
                        Self::FIELDS.url,
                        format!("invalid URL: {}", e),
                        "use format like https://example.com",
                    );
                }
            }
        }
    }
}
