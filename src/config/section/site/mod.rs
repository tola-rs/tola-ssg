//! `[site]` section configuration.
//!
//! Contains site metadata and navigation settings.
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
//! ```

mod info;
mod nav;

pub use info::SiteInfoConfig;
pub use nav::NavConfig;

use macros::Config;
use serde::{Deserialize, Serialize};

/// Site section configuration containing info and nav.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site")]
pub struct SiteSectionConfig {
    /// Site metadata (title, author, description, etc.)
    #[config(sub_config)]
    pub info: SiteInfoConfig,

    /// SPA navigation settings (includes transition and preload).
    #[config(sub_config)]
    pub nav: NavConfig,
}
