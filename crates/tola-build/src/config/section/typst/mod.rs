//! `[typst]` section configuration.
//!
//! The Typst compiler's input on this host: the directory it discovers fonts in.

use serde::{Deserialize, Serialize};
use tola_config::Config;

pub mod fonts;

pub use fonts::FontsConfig;

/// How the Typst compiler finds the fonts it can use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "typst")]
pub struct TypstSectionConfig {
    #[config(sub)]
    pub fonts: FontsConfig,
}
