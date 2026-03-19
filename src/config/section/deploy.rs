//! `[deploy]` section configuration. (WIP, only github page supported now)
//!
//! Contains deployment settings for various providers (GitHub, Cloudflare, Vercel).
//!
//! # Example
//!
//! ```toml
//! [deploy]
//! provider = "github"         # Deployment provider: github | cloudflare | vercel
//! force = false               # Force push (overwrites remote history)
//!
//! [deploy.github]
//! url = "https://github.com/user/user.github.io"  # Repository URL
//! branch = "gh-pages"                              # Target branch
//! token_path = "~/.github-token"                   # Optional: PAT file path
//! ```

use macros::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Deploy configuration (not implemented)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy", status = not_implemented)]
pub struct DeployConfig {
    /// Deployment provider: "github", "cloudflare", "vercel".
    pub provider: String,

    /// Force push (overwrites remote history).
    pub force: bool,

    /// GitHub Pages deployment settings.
    #[config(sub)]
    pub github: GithubDeployConfig,

    /// Cloudflare Pages settings (not yet implemented).
    #[config(sub)]
    pub cloudflare: CloudflareDeployConfig,

    /// Vercel settings (not yet implemented).
    #[config(sub)]
    pub vercel: VercelDeployConfig,
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            provider: "github".to_string(),
            force: false,
            github: GithubDeployConfig::default(),
            cloudflare: CloudflareDeployConfig::default(),
            vercel: VercelDeployConfig::default(),
        }
    }
}

impl DeployConfig {
    /// Validate deploy configuration.
    ///
    /// # Checks
    /// - If `github.token_path` is set, it must exist and be a file.
    pub fn validate(&self, diag: &mut crate::config::ConfigDiagnostics) {
        if let Some(path) = &self.github.token_path {
            if !path.exists() {
                diag.error(
                    GithubDeployConfig::FIELDS.token_path,
                    format!(
                        "{} file not found: {}",
                        GithubDeployConfig::FIELDS.token_path,
                        path.display()
                    ),
                );
            } else if !path.is_file() {
                diag.error(
                    GithubDeployConfig::FIELDS.token_path,
                    format!(
                        "{} is not a file: {}",
                        GithubDeployConfig::FIELDS.token_path,
                        path.display()
                    ),
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy.github", status = not_implemented)]
pub struct GithubDeployConfig {
    /// Repository URL (HTTPS or SSH format).
    pub url: String,

    /// Target branch for deployment (e.g., "main", "gh-pages").
    pub branch: String,

    /// Path to file containing GitHub personal access token.
    ///
    /// # Security
    /// - Store outside repository (e.g., `~/.github-token`)
    /// - Never commit tokens to version control!
    pub token_path: Option<PathBuf>,
}

impl Default for GithubDeployConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            branch: "main".to_string(),
            token_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy.cloudflare", status = not_implemented)]
pub struct CloudflareDeployConfig {
    /// Provider identifier
    pub provider: String,
}

impl Default for CloudflareDeployConfig {
    fn default() -> Self {
        Self {
            provider: "github".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy.vercel", status = not_implemented)]
pub struct VercelDeployConfig {
    /// Provider identifier
    pub provider: String,
}

impl Default for VercelDeployConfig {
    fn default() -> Self {
        Self {
            provider: "github".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{ConfigDiagnostics, ConfigPresence, SiteConfig, test_parse_config};

    #[test]
    fn test_deploy_unknown_fields_detected() {
        for content in [
            "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[deploy]\nunknown = \"field\"",
            "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[deploy.github]\nunknown = \"field\"",
        ] {
            let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
            assert!(ignored.iter().any(|f| f.contains("unknown")));
        }
    }

    #[test]
    fn test_not_implemented_section_triggers_on_explicit_presence_even_if_default() {
        let snippet = r#"
[deploy.cloudflare]
provider = "github"
"#;
        let config = test_parse_config(snippet);
        let mut diag = ConfigDiagnostics::new();
        let raw = format!("[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n{snippet}");
        diag.set_presence(ConfigPresence::from_toml(&raw).unwrap());

        config.deploy.validate_field_status(&mut diag);
        assert!(diag.has_errors());
    }
}
