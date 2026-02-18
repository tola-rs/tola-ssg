//! Sitemap generation configuration.

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.sitemap")]
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
