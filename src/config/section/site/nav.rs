//! `[site.nav]` configuration for SPA navigation.
//!
//! # Example
//!
//! ```toml
//! [site.nav]
//! spa = true
//! transition = { style = "fade", time = 200 }
//! preload = { enable = true, delay = 100 }
//! ```

use macros::Config;
use serde::{Deserialize, Serialize};

/// SPA navigation configuration
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.nav")]
pub struct NavConfig {
    /// Enable SPA navigation (link interception + DOM morphing).
    pub spa: bool,

    /// View transition settings.
    #[config(skip)]
    pub transition: TransitionConfig,

    /// Preload/prefetch settings.
    #[config(skip)]
    pub preload: PreloadConfig,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            spa: true,
            transition: TransitionConfig::default(),
            preload: PreloadConfig::default(),
        }
    }
}

/// View Transitions API configuration
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.nav.transition")]
pub struct TransitionConfig {
    /// Transition style: "none" or "fade".
    /// Setting to "fade" enables View Transitions API.
    pub style: TransitionStyle,

    /// Transition duration in milliseconds.
    pub time: u32,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            style: TransitionStyle::None,
            time: 200,
        }
    }
}

impl TransitionConfig {
    /// Returns true if View Transitions are enabled (style != None).
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.style != TransitionStyle::None
    }
}

/// Transition style for page navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitionStyle {
    /// No transition animation (instant swap).
    #[default]
    None,

    /// Fade transition using View Transitions API.
    Fade,
}

/// Preload/prefetch configuration
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "site.nav.preload")]
pub struct PreloadConfig {
    /// Enable hover-based prefetching.
    pub enable: bool,

    /// Delay in milliseconds before prefetching (to avoid false triggers).
    pub delay: u32,
}

impl Default for PreloadConfig {
    fn default() -> Self {
        Self {
            enable: false,
            delay: 100,
        }
    }
}
