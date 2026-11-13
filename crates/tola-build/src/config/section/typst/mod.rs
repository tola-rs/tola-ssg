//! `[typst]` section configuration.
//!
//! The Typst compiler's input on this host: the directory it discovers fonts in.

use serde::{Deserialize, Serialize};
use tola_config::Config;

pub mod fonts;

pub use fonts::FontsConfig;

/// Font settings for the Typst compiler. See `[typst.fonts]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "typst")]
pub struct TypstSectionConfig {
    #[config(sub)]
    pub fonts: FontsConfig,
}
