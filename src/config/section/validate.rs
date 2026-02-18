//! `[validate]` section configuration.
//!
//! Configuration for the `tola validate` command to check links and assets.
//!
//! # Example
//!
//! ```toml
//! [validate.pages]
//! enable = true               # Check internal page links
//! level = "error"             # Failure level: error | warn
//!
//! [validate.assets]
//! enable = true               # Check referenced assets exist
//! level = "error"             # Failure level: error | warn
//! ```

use macros::Config;
use serde::{Deserialize, Serialize};

// ============================================================================
// Main ValidateConfig
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate")]
pub struct ValidateConfig {
    /// Page link validation settings.
    #[config(sub)]
    pub pages: PagesValidateConfig,

    /// Asset validation settings.
    #[config(sub)]
    pub assets: AssetsValidateConfig,
}

// ============================================================================
// Pages Validation
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate.pages")]
pub struct PagesValidateConfig {
    /// Enable page link validation.
    pub enable: bool,

    /// How to treat validation failures: "error" or "warn".
    pub level: ValidateLevel,
}

impl Default for PagesValidateConfig {
    fn default() -> Self {
        Self {
            enable: true,
            level: ValidateLevel::default(),
        }
    }
}

// ============================================================================
// Assets Validation
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate.assets")]
pub struct AssetsValidateConfig {
    /// Enable asset validation.
    pub enable: bool,

    /// How to treat validation failures: "error" or "warn".
    pub level: ValidateLevel,
}

impl Default for AssetsValidateConfig {
    fn default() -> Self {
        Self {
            enable: true,
            level: ValidateLevel::default(),
        }
    }
}

/// Validation error level
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ValidateLevel {
    /// Treat validation failures as errors (build fails).
    #[default]
    Error,
    /// Treat validation failures as warnings (build continues).
    Warn,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SiteConfig, test_parse_config};

    #[test]
    fn test_validate_config_defaults() {
        let config = test_parse_config("");
        // pages and assets are enabled by default
        assert!(config.validate.pages.enable);
        assert!(config.validate.assets.enable);
    }

    #[test]
    fn test_validate_config_custom() {
        let config = test_parse_config(
            r#"[validate.pages]
enable = true
level = "warn"

[validate.assets]
enable = false
level = "warn""#,
        );
        assert!(config.validate.pages.enable);
        assert!(!config.validate.assets.enable);
        assert!(matches!(
            config.validate.pages.level,
            ValidateLevel::Warn
        ));
    }

    #[test]
    fn test_validate_unknown_field_detected() {
        let content = "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[validate]\nunknown = \"field\"";
        let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
        assert!(ignored.iter().any(|f| f.contains("unknown")));
    }
}
