//! `[theme.recolor]` configuration for image color adaptation.
//!
//! # Example
//!
//! ```toml
//! [theme.recolor]
//! enable = true
//! source = "auto"  # "auto" | "--css-var" | "static"
//!
//! # When source = "static"
//! [theme.recolor.list]
//! light = "#000000"
//! dark = "#ffffff"
//! nord = "#88c0d0"
//! ```

use macros::Config;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Recolor configuration for image color adaptation.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "theme.recolor")]
pub struct RecolorConfig {
    /// Enable recolor functionality.
    pub enable: bool,

    /// Color source: "auto" | "--css-var-name" | "static".
    /// - "auto": Read `--tola-recolor-value` or fallback to `body { color }`
    /// - "--var": Read specified CSS variable
    /// - "static": Use colors from `list`
    pub source: RecolorSource,

    /// Target selection: "manual" | "auto".
    /// - "manual": User manually adds `.tola-recolor` class
    /// - "auto": Automatically inject `.tola-recolor` to all `<img>` elements
    pub target: RecolorTarget,

    /// Static color definitions (used when source = "static").
    /// Key is theme name, value is hex color.
    #[config(skip)]
    pub list: HashMap<String, String>,
}

impl Default for RecolorConfig {
    fn default() -> Self {
        Self {
            enable: false,
            source: RecolorSource::Auto,
            target: RecolorTarget::Manual,
            list: HashMap::new(),
        }
    }
}

/// Recolor target selection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecolorTarget {
    /// User manually adds `.tola-recolor` class.
    #[default]
    Manual,
    /// Automatically inject `.tola-recolor` to all `<img>` elements.
    Auto,
}

/// Recolor color source.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecolorSource {
    /// Auto-detect: `--tola-recolor-value` → `body { color }`.
    #[default]
    Auto,
    /// Use static colors from `list`.
    Static,
    /// Read from specified CSS variable (e.g., "--text-color").
    /// Must be placed last due to #[serde(untagged)].
    #[serde(untagged)]
    CssVar(String),
}
