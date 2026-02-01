//! rss feed generation.
//!
//! Parses post metadata and generates rss/atom feeds.

use crate::{
    compiler::meta::{PageMeta, Pages},
    config::SiteConfig,
    log,
    utils::{
        date::DateTimeUtc,
        minify::{MinifyType, minify},
    },
};
use anyhow::{Ok, Result, anyhow};
use regex::Regex;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};
use std::{fs, sync::LazyLock};

// ============================================================================
// Public API
// ============================================================================

/// Build rss feed if enabled in config.
pub fn build_rss(config: &SiteConfig, pages: &Pages) -> Result<()> {
    if config.build.rss.enable {
        RssFeed::build(config, pages)?.write(config)?;
    }
    Ok(())
}

// ============================================================================
// RssFeed Implementation
// ============================================================================

/// rss feed builder
struct RssFeed<'a> {
    config: &'a SiteConfig,
    pages: Vec<&'a PageMeta>,
}

impl<'a> RssFeed<'a> {
    /// Build rss feed using pre-collected page metadata.
    ///
    /// Pages without content metadata are silently skipped.
    fn build(config: &'a SiteConfig, pages: &'a Pages) -> Result<Self> {
        let pages: Vec<_> = pages.iter().filter(|p| p.content_meta.is_some()).collect();

        Ok(Self { config, pages })
    }

    /// Generate rss xml string
    fn into_xml(self) -> Result<String> {
        let items: Vec<_> = self
            .pages
            .iter()
            .filter_map(|page| page_to_rss_item(page, self.config))
            .collect();

        let channel = ChannelBuilder::default()
            .title(&self.config.base.title)
            .link(self.config.base.url.as_deref().unwrap_or_default())
            .description(&self.config.base.description)
            .language(self.config.base.language.clone())
            .generator("tola-ssg".to_string())
            .items(items)
            .build();

        channel
            .validate()
            .map_err(|e| anyhow!("rss validation failed: {e}"))?;
        Ok(channel.to_string())
    }

    /// Write rss feed to file
    fn write(self, config: &SiteConfig) -> Result<()> {
        let xml = self.into_xml()?;
        let xml = minify(MinifyType::Xml(xml.as_bytes()), config);
        // Resolve RSS path relative to output_dir (with path_prefix)
        let rss_path = config.paths().output_dir().join(&config.build.rss.path);

        if let Some(parent) = rss_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&rss_path, &*xml)?;

        log!("rss"; "{}", rss_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert `PageMeta` to rss item.
/// Returns None if required fields (title, date) are missing.
fn page_to_rss_item(page: &PageMeta, config: &SiteConfig) -> Option<rss::Item> {
    let content = page.content_meta.as_ref()?;
    let title = content.title.clone()?;
    let date = content.date.as_deref()?;
    let pub_date = DateTimeUtc::parse(date).map(DateTimeUtc::to_rfc2822)?;
    let link = page.paths.full_url.clone();
    let author = normalize_rss_author(content.author.as_ref(), config);

    Some(
        ItemBuilder::default()
            .title(title)
            .link(Some(link.clone()))
            .guid(GuidBuilder::default().permalink(true).value(link).build())
            .description(content.summary.clone())
            .pub_date(pub_date)
            .author(author)
            .build(),
    )
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Normalize author field to rss format: "email@example.com (Name)"
///
/// Priority:
/// 1. Post meta author if already in valid format
/// 2. Site config author if in valid format
/// 3. Combine site config email and author
fn normalize_rss_author(author: Option<&String>, config: &SiteConfig) -> Option<String> {
    static RE_VALID_AUTHOR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}[ \t]*\([^)]+\)$").unwrap()
    });

    let author = author?;

    // Check if post author is already valid
    if RE_VALID_AUTHOR.is_match(author) {
        return Some(author.clone());
    }

    // Try site config author
    let site_author = &config.base.author;
    if RE_VALID_AUTHOR.is_match(site_author) {
        return Some(site_author.clone());
    }

    // Combine email and author name
    Some(format!("{} ({})", config.base.email, site_author))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::meta::{ContentMeta, PagePaths};
    use std::path::PathBuf;

    // Helper to create a config for testing
    fn make_config(author: &str, email: &str) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.base.author = author.to_string();
        config.base.email = email.to_string();
        config.base.url = Some("https://example.com".to_string());
        config
    }

    fn make_page(title: &str, date: &str, summary: Option<&str>, author: Option<&str>) -> PageMeta {
        PageMeta {
            paths: PagePaths {
                source: PathBuf::from("test.typ"),
                html: PathBuf::from("public/test/index.html"),
                relative: "test".to_string(),
                url_path: "/test/".to_string(),
                full_url: "https://example.com/test/".to_string(),
            },
            lastmod: None,
            content_meta: Some(ContentMeta {
                url: None,
                title: Some(title.to_string()),
                summary: summary.map(String::from),
                date: Some(date.to_string()),
                update: None,
                author: author.map(String::from),
                draft: false,
                tags: vec![],
            }),
            compiled_html: None,
        }
    }

    #[test]
    fn test_normalize_rss_author() {
        let config = make_config("Site Author", "site@example.com");

        // Case 1: Post author is already valid
        let post_author = "post@example.com (Post Author)".to_string();
        assert_eq!(
            normalize_rss_author(Some(&post_author), &config),
            Some(post_author)
        );

        // Case 2: Post author is invalid (just name), fallback to site config (combined)
        let post_author_invalid = "Post Author".to_string();
        assert_eq!(
            normalize_rss_author(Some(&post_author_invalid), &config),
            Some("site@example.com (Site Author)".to_string())
        );

        // Case 3: Post author None, returns None (current behavior)
        assert_eq!(normalize_rss_author(None, &config), None);

        // Case 4: Site author is valid email format
        let config_valid = make_config("site@example.com (Site Author)", "");
        assert_eq!(
            normalize_rss_author(Some(&post_author_invalid), &config_valid),
            Some("site@example.com (Site Author)".to_string())
        );
    }

    #[test]
    fn test_page_to_rss_item() {
        let config = make_config("Site Author", "site@example.com");
        let page = make_page(
            "Test Title",
            "2024-01-01T00:00:00Z",
            Some("Test Summary"),
            Some("author@example.com (Author)"),
        );

        let item = page_to_rss_item(&page, &config).expect("Should convert to RSS item");
        assert_eq!(item.title(), Some("Test Title"));
        assert_eq!(item.link(), Some("https://example.com/test/"));
        assert_eq!(item.description(), Some("Test Summary"));
        assert_eq!(item.author(), Some("author@example.com (Author)"));
        // RFC2822 format check
        assert!(item.pub_date().unwrap().contains("Jan 2024"));
    }

    #[test]
    fn test_page_to_rss_item_missing_title() {
        let config = make_config("Site Author", "site@example.com");
        let mut page = make_page("Title", "2024-01-01", None, None);
        page.content_meta.as_mut().unwrap().title = None;

        assert!(page_to_rss_item(&page, &config).is_none());
    }

    #[test]
    fn test_page_to_rss_item_missing_date() {
        let config = make_config("Site Author", "site@example.com");
        let mut page = make_page("Title", "2024-01-01", None, None);
        page.content_meta.as_mut().unwrap().date = None;

        assert!(page_to_rss_item(&page, &config).is_none());
    }
}
