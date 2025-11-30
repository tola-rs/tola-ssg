//! `[base]` section configuration.
//!
//! Contains basic site information like title, author, description, etc.

use super::defaults;
use educe::Educe;
use serde::{Deserialize, Serialize};

/// `[base]` section in tola.toml - basic site metadata.
///
/// # Example
/// ```toml
/// [base]
/// title = "My Blog"
/// description = "A personal blog about Rust"
/// author = "Alice"
/// url = "https://myblog.com"
/// ```
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct BaseConfig {
    /// Site title displayed in browser tab and headers.
    pub title: String,

    /// Author name for RSS feed and meta tags.
    #[serde(default = "defaults::base::author")]
    #[educe(Default = defaults::base::author())]
    pub author: String,

    /// Author email for RSS feed.
    #[serde(default = "defaults::base::email")]
    #[educe(Default = defaults::base::email())]
    pub email: String,

    /// Site description for SEO meta tags.
    pub description: String,

    /// Base URL for absolute links in RSS/sitemap.
    /// Required when `[build.rss].enable = true`.
    #[serde(default = "defaults::base::url")]
    #[educe(Default = defaults::base::url())]
    pub url: Option<String>,

    /// BCP 47 language code (e.g., "zh-Hans", "en-US").
    #[serde(default = "defaults::base::language")]
    #[educe(Default = defaults::base::language())]
    pub language: String,

    /// Copyright notice for site footer.
    #[serde(default)]
    pub copyright: String,
}

#[cfg(test)]
mod tests {
    use super::super::SiteConfig;

    #[test]
    fn test_base_config_full() {
        let config = r#"
            [base]
            title = "KawaYww"
            description = "KawaYww's Blog"
            url = "https://kawayww.com"
            language = "zh_Hans"
            copyright = "2025 KawaYww"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.base.title, "KawaYww");
        assert_eq!(config.base.description, "KawaYww's Blog");
        assert_eq!(config.base.url, Some("https://kawayww.com".to_string()));
        assert_eq!(config.base.language, "zh_Hans");
        assert_eq!(config.base.copyright, "2025 KawaYww");
    }

    #[test]
    fn test_base_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.base.author, "<YOUR_NAME>");
        assert_eq!(config.base.email, "user@noreply.tola");
        assert_eq!(config.base.language, "zh-Hans");
        assert_eq!(config.base.url, None);
        assert_eq!(config.base.copyright, "");
    }

    #[test]
    fn test_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
            unknown_field = "should_fail"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown field"));
    }

    #[test]
    fn test_base_config_author_email() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
            author = "Alice"
            email = "alice@example.com"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.base.author, "Alice");
        assert_eq!(config.base.email, "alice@example.com");
    }

    #[test]
    fn test_base_config_url_with_path() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
            url = "https://example.com/blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(
            config.base.url,
            Some("https://example.com/blog".to_string())
        );
    }

    #[test]
    fn test_base_config_empty_strings() {
        let config = r#"
            [base]
            title = ""
            description = ""
            author = ""
            copyright = ""
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.base.title, "");
        assert_eq!(config.base.description, "");
        assert_eq!(config.base.author, "");
        assert_eq!(config.base.copyright, "");
    }

    #[test]
    fn test_base_config_unicode() {
        let config = r#"
            [base]
            title = "我的博客"
            description = "这是一个中文博客"
            author = "张三"
            language = "zh-Hans"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.base.title, "我的博客");
        assert_eq!(config.base.description, "这是一个中文博客");
        assert_eq!(config.base.author, "张三");
    }
}
