//! Sitemap generation configuration.

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sitemap generation settings.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.sitemap")]
pub struct SitemapConfig {
    /// Enable sitemap generation.
    #[config(inline_doc)]
    pub enable: bool,
    /// Output path for sitemap file.
    #[config(inline_doc)]
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
