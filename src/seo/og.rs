//! Open Graph and Twitter Card meta tags data.
//!
//! Provides pure data structures for OG tags generation.
//! VDOM injection is handled by `pipeline/transform/header.rs`.

use crate::config::SiteConfig;

/// Default Open Graph tags from site config.
///
/// Only contains site-level defaults. Page-specific tags (og:title, og:url)
/// should be set via Typst `og-tags()` function.
pub struct OgDefaults<'a> {
    pub og_type: &'static str,
    pub site_name: &'a str,
    pub locale: &'a str,
    pub description: &'a str,
    pub twitter_card: &'static str,
}

impl<'a> OgDefaults<'a> {
    /// Create default OG tags from site config.
    pub fn from_config(config: &'a SiteConfig) -> Self {
        Self {
            og_type: "website",
            site_name: &config.site.info.title,
            locale: &config.site.info.language,
            description: &config.site.info.description,
            twitter_card: "summary_large_image",
        }
    }
}
