//! `[build.css]` section configuration.

use macros::Config;
use serde::{Deserialize, Serialize};

use super::atomic::AtomicCssConfig;

/// CSS build settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.css")]
pub struct CssConfig {
    /// Native atomic CSS generation.
    pub atomic: AtomicCssConfig,
}
