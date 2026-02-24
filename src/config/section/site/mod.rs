//! `[site]` section configuration.
//!
//! Contains site metadata, navigation, and site-level features.
//!
//! # Example
//!
//! ```toml
//! [site.info]
//! title = "My Blog"
//! description = "A personal blog"
//! author = "Alice"
//! url = "https://myblog.com"
//!
//! [site.info.extra]
//! github = "https://github.com/alice"
//!
//! [site.nav]
//! enable = true
//! transition = { style = "fade", time = 200 }
//! preload = { enable = true, delay = 100 }
//!
//! [site.header]
//! icon = "favicon.ico"
//! styles = ["styles/custom.css"]
//! scripts = ["scripts/app.js"]
//!
//! [site.seo]
//! auto_og = true
//!
//! [site.seo.feed]
//! enable = true
//! path = "feed.xml"
//!
//! [site.seo.sitemap]
//! enable = true
//!
//! [site]
//! not_found = "404.html"
//! ```

mod header;
mod info;
mod nav;
mod seo;

pub use header::HeaderConfig;
pub use info::SiteInfoConfig;
pub use nav::{NavConfig, TransitionStyle};
pub use seo::{FeedConfig, FeedFormat, SeoConfig};

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Site section configuration containing info, nav, and site-level features
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site")]
pub struct SiteSectionConfig {
    /// Site metadata (title, author, description, etc.)
    #[config(sub)]
    pub info: SiteInfoConfig,

    /// SPA navigation settings (includes transition and preload).
    #[config(sub)]
    pub nav: NavConfig,

    /// Custom `<head>` elements (favicon, styles, scripts).
    #[config(sub)]
    pub header: HeaderConfig,

    /// SEO settings (feed, sitemap, OG tags).
    #[config(sub)]
    pub seo: SeoConfig,

    /// Custom 404 page source file (relative to site root).
    pub not_found: Option<PathBuf>,
}
