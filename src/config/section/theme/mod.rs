//! `[theme]` section configuration.
//!
//! Contains theme-related settings like recolor.
//!
//! # Example
//!
//! ```toml
//! [theme.recolor]
//! enable = true
//! source = "auto"
//! ```

mod recolor;

pub use recolor::{RecolorConfig, RecolorSource, RecolorTarget};

use macros::Config;
use serde::{Deserialize, Serialize};

/// Theme section configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "theme")]
pub struct ThemeSectionConfig {
    /// Image recolor settings.
    #[config(sub)]
    pub recolor: RecolorConfig,
}
