//! `[deploy]` section configuration.
//!
//! Contains deployment settings for various providers (GitHub, Cloudflare, Vercel).

use super::defaults;
use educe::Educe;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `[deploy]` section in tola.toml - deployment configuration.
///
/// # Example
/// ```toml
/// [deploy]
/// provider = "github"
/// force = false
///
/// [deploy.github]
/// url = "https://github.com/user/user.github.io"
/// branch = "main"
/// ```
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct DeployConfig {
    /// Deployment provider: "github", "cloudflare", "vercel".
    #[serde(default = "defaults::deploy::provider")]
    #[educe(Default = defaults::deploy::provider())]
    pub provider: String,

    /// Force push (overwrites remote history).
    #[serde(default = "defaults::r#false")]
    #[educe(Default = defaults::r#false())]
    pub force: bool,

    /// GitHub Pages deployment settings.
    #[serde(default)]
    pub github: GithubDeployConfig,

    /// Cloudflare Pages settings (not yet implemented).
    #[serde(default)]
    pub cloudflare: CloudflareDeployConfig,

    /// Vercel settings (not yet implemented).
    #[serde(default)]
    pub vercel: VercelDeployConfig,
}

/// `[deploy.github]` section - GitHub Pages deployment.
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct GithubDeployConfig {
    /// Repository URL (HTTPS or SSH format).
    #[serde(default = "defaults::deploy::github::url")]
    #[educe(Default = defaults::deploy::github::url())]
    pub url: String,

    /// Target branch for deployment (e.g., "main", "gh-pages").
    #[serde(default = "defaults::deploy::github::branch")]
    #[educe(Default = defaults::deploy::github::branch())]
    pub branch: String,

    /// Path to file containing GitHub personal access token.
    ///
    /// # Security
    /// - Store outside repository (e.g., `~/.github-token`)
    /// - Never commit tokens to version control!
    #[serde(default = "defaults::deploy::github::token_path")]
    #[educe(Default = defaults::deploy::github::token_path())]
    pub token_path: Option<PathBuf>,
}

/// `[deploy.cloudflare]` section (placeholder for future implementation)
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct CloudflareDeployConfig {
    /// Provider identifier
    #[serde(default = "defaults::deploy::provider")]
    #[educe(Default = defaults::deploy::provider())]
    pub provider: String,
}

/// `[deploy.vercel]` section (placeholder for future implementation)
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct VercelDeployConfig {
    /// Provider identifier
    #[serde(default = "defaults::deploy::provider")]
    #[educe(Default = defaults::deploy::provider())]
    pub provider: String,
}

#[cfg(test)]
mod tests {
    use super::super::SiteConfig;
    use std::path::PathBuf;

    #[test]
    fn test_deploy_config() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [deploy]
            provider = "github"
            force = true

            [deploy.github]
            url = "https://github.com/user/user.github.io"
            branch = "gh-pages"
            token_path = "~/.github-token"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

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
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.deploy.provider, "github");
        assert!(!config.deploy.force);
        assert_eq!(config.deploy.github.branch, "main");
        assert!(config.deploy.github.token_path.is_none());
    }

    #[test]
    fn test_deploy_config_github_custom_branch() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.github]
            branch = "gh-pages"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.deploy.github.branch, "gh-pages");
    }

    #[test]
    fn test_deploy_config_github_url_variations() {
        // HTTPS URL
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.github]
            url = "https://github.com/user/repo.git"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.deploy.github.url, "https://github.com/user/repo.git");

        // SSH URL
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.github]
            url = "git@github.com:user/repo.git"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.deploy.github.url, "git@github.com:user/repo.git");
    }

    #[test]
    fn test_deploy_config_force_flag() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy]
            force = true
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(config.deploy.force);
    }

    #[test]
    fn test_deploy_config_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_config_github_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.github]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_config_cloudflare_placeholder() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.cloudflare]
            provider = "cloudflare"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.deploy.cloudflare.provider, "cloudflare");
    }

    #[test]
    fn test_deploy_config_vercel_placeholder() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [deploy.vercel]
            provider = "vercel"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.deploy.vercel.provider, "vercel");
    }
}
