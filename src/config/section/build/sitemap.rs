//! Sitemap generation configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SitemapConfig {
    /// Enable sitemap generation.
    pub enable: bool,
    /// Output path for sitemap file.
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
