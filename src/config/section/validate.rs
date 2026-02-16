//! `[validate]` section configuration.
//!
//! Configuration for the `tola validate` command to check links and assets.
//!
//! # Example
//!
//! ```toml
//! [validate.link.external]
//! enable = true               # Check external HTTP/HTTPS links
//! timeout = 10                # HTTP request timeout in seconds
//! concurrency = 20            # Max concurrent HTTP requests
//! skip_prefixes = ["http://localhost"]  # URLs to skip
//! level = "error"             # Failure level: error | warn
//!
//! [validate.link.internal]
//! enable = true               # Check internal site links
//! fragments = false           # Also validate anchor fragments
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
    /// Link validation settings (internal and external).
    #[config(sub_config)]
    pub link: LinkValidateConfig,

    /// Asset validation settings.
    #[config(sub_config)]
    pub assets: AssetsValidateConfig,
}

// ============================================================================
// Link Validation
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate.link")]
pub struct LinkValidateConfig {
    /// External link validation (HTTP/HTTPS).
    pub external: ExternalLinkConfig,

    /// Internal link validation (site pages).
    pub internal: InternalLinkConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate.link.external")]
pub struct ExternalLinkConfig {
    /// Enable external link validation.
    pub enable: bool,

    /// Timeout for HTTP requests in seconds.
    pub timeout: u64,

    /// Maximum number of concurrent HTTP requests.
    pub concurrency: usize,

    /// URL prefixes to skip during validation.
    pub skip_prefixes: Vec<String>,

    /// How to treat validation failures: "error" or "warn".
    pub level: ValidateLevel,
}

impl Default for ExternalLinkConfig {
    fn default() -> Self {
        Self {
            enable: false,
            timeout: 10,
            concurrency: 20,
            skip_prefixes: Vec::new(),
            level: ValidateLevel::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "validate.link.internal")]
pub struct InternalLinkConfig {
    /// Enable internal link validation.
    pub enable: bool,

    /// How to treat validation failures: "error" or "warn".
    pub level: ValidateLevel,

    /// Validate fragment anchors (requires heading index).
    pub fragments: bool,
}

impl Default for InternalLinkConfig {
    fn default() -> Self {
        Self {
            enable: true,
            level: ValidateLevel::default(),
            fragments: false,
        }
    }
}

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

/// Validation error level.
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
        // external is disabled by default
        assert!(!config.validate.link.external.enable);
        // internal and assets are enabled by default
        assert!(config.validate.link.internal.enable);
        assert!(config.validate.assets.enable);
    }

    #[test]
    fn test_validate_config_custom() {
        let config = test_parse_config(
            r#"[validate.link.external]
enable = true
timeout = 30
concurrency = 10
skip_prefixes = ["http://localhost"]
level = "warn"

[validate.link.internal]
enable = true
level = "warn"
fragments = true

[validate.assets]
enable = false
level = "warn""#,
        );
        assert!(config.validate.link.external.enable);
        assert!(config.validate.link.internal.enable);
        assert!(!config.validate.assets.enable);
        assert_eq!(config.validate.link.external.timeout, 30);
        assert_eq!(config.validate.link.external.concurrency, 10);
        assert_eq!(config.validate.link.external.skip_prefixes.len(), 1);
        assert!(matches!(
            config.validate.link.external.level,
            ValidateLevel::Warn
        ));
        assert!(config.validate.link.internal.fragments);
    }

    #[test]
    fn test_validate_unknown_field_detected() {
        let content = "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[validate]\nunknown = \"field\"";
        let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
        assert!(ignored.iter().any(|f| f.contains("unknown")));
    }
}
