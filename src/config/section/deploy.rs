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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use macros::Config;

/// Deploy configuration (experimental).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy", status = experimental)]
pub struct DeployConfig {
    /// Deployment provider: "github", "cloudflare", "vercel".
    pub provider: String,

    /// Force push (overwrites remote history).
    pub force: bool,

    /// GitHub Pages deployment settings.
    #[config(sub_config)]
    pub github: GithubDeployConfig,

    /// Cloudflare Pages settings (not yet implemented).
    #[config(sub_config)]
    pub cloudflare: CloudflareDeployConfig,

    /// Vercel settings (not yet implemented).
    #[config(sub_config)]
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
                    format!("{} file not found: {}", GithubDeployConfig::FIELDS.token_path, path.display()),
                );
            } else if !path.is_file() {
                diag.error(
                    GithubDeployConfig::FIELDS.token_path,
                    format!("{} is not a file: {}", GithubDeployConfig::FIELDS.token_path, path.display()),
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "deploy.github", status = experimental)]
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
    use crate::config::{test_parse_config, SiteConfig};
    use std::path::PathBuf;

    #[test]
    fn test_deploy_config() {
        let config = test_parse_config(
            r#"[deploy]
provider = "github"
force = true

[deploy.github]
url = "https://github.com/user/user.github.io"
branch = "gh-pages"
token_path = "~/.github-token""#,
        );

        assert_eq!(config.deploy.provider, "github");
        assert!(config.deploy.force);
        assert_eq!(
            config.deploy.github.url,
            "https://github.com/user/user.github.io"
        );
        assert_eq!(config.deploy.github.branch, "gh-pages");
        assert_eq!(
            config.deploy.github.token_path,
            Some(PathBuf::from("~/.github-token"))
        );
    }

    #[test]
    fn test_deploy_config_defaults() {
        let config = test_parse_config("");

        assert_eq!(config.deploy.provider, "github");
        assert!(!config.deploy.force);
        assert_eq!(config.deploy.github.branch, "main");
        assert!(config.deploy.github.token_path.is_none());
    }

    #[test]
    fn test_deploy_config_github_custom_branch() {
        let config = test_parse_config("[deploy.github]\nbranch = \"gh-pages\"");
        assert_eq!(config.deploy.github.branch, "gh-pages");
    }

    #[test]
    fn test_deploy_config_github_url_variations() {
        // HTTPS URL
        let config =
            test_parse_config("[deploy.github]\nurl = \"https://github.com/user/repo.git\"");
        assert_eq!(config.deploy.github.url, "https://github.com/user/repo.git");

        // SSH URL
        let config = test_parse_config("[deploy.github]\nurl = \"git@github.com:user/repo.git\"");
        assert_eq!(config.deploy.github.url, "git@github.com:user/repo.git");
    }

    #[test]
    fn test_deploy_config_force_flag() {
        let config = test_parse_config("[deploy]\nforce = true");
        assert!(config.deploy.force);
    }

    #[test]
    fn test_deploy_unknown_field_detected() {
        let content = "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[deploy]\nunknown = \"field\"";
        let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
        assert!(ignored.iter().any(|f| f.contains("unknown")));
    }

    #[test]
    fn test_deploy_github_unknown_field_detected() {
        let content =
            "[site.info]\ntitle = \"Test\"\ndescription = \"Test\"\n[deploy.github]\nunknown = \"field\"";
        let (_, ignored) = SiteConfig::parse_with_ignored(content).unwrap();
        assert!(ignored.iter().any(|f| f.contains("unknown")));
    }

    #[test]
    fn test_deploy_config_cloudflare_placeholder() {
        let config = test_parse_config("[deploy.cloudflare]\nprovider = \"cloudflare\"");
        assert_eq!(config.deploy.cloudflare.provider, "cloudflare");
    }

    #[test]
    fn test_deploy_config_vercel_placeholder() {
        let config = test_parse_config("[deploy.vercel]\nprovider = \"vercel\"");
        assert_eq!(config.deploy.vercel.provider, "vercel");
    }
}
