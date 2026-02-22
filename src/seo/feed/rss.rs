//! RSS 2.0 feed generation.
//!
//! Generates RSS feeds from page metadata stored in GLOBAL_SITE_DATA.

use super::common::{FeedPage, get_feed_pages};
use crate::{config::SiteConfig, seo::minify_xml, log, utils::date::DateTimeUtc};
use anyhow::{Ok, Result, anyhow};
use regex::Regex;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};
use std::{fs, sync::LazyLock};

/// Build RSS 2.0 feed
pub fn build_rss(config: &SiteConfig) -> Result<()> {
    RssFeed::build(config).write()
}

struct RssFeed {
    config: SiteConfig,
    pages: Vec<FeedPage>,
}

impl RssFeed {
    fn build(config: &SiteConfig) -> Self {
        let pages = get_feed_pages();
        Self {
            config: config.clone(),
            pages,
        }
    }

    fn into_xml(self) -> Result<String> {
        let items: Vec<_> = self
            .pages
            .iter()
            .filter_map(|page| page_to_rss_item(page, &self.config))
            .collect();

        let channel = ChannelBuilder::default()
            .title(&self.config.site.info.title)
            .link(self.config.site.info.url.as_deref().unwrap_or_default())
            .description(&self.config.site.info.description)
            .language(self.config.site.info.language.clone())
            .generator("tola-ssg".to_string())
            .items(items)
            .build();

        channel
            .validate()
            .map_err(|e| anyhow!("RSS validation failed: {e}"))?;
        Ok(channel.to_string())
    }

    fn write(self) -> Result<()> {
        let minify = self.config.build.minify;
        let output_dir = self.config.paths().output_dir();
        let feed_path = self.config.site.feed.path.clone();
        let xml = self.into_xml()?;
        let xml = minify_xml(xml.as_bytes(), minify);
        // Resolve feed path relative to output_dir (with path_prefix)
        let rss_path = output_dir.join(&feed_path);

        if let Some(parent) = rss_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&rss_path, &*xml)?;

        log!("rss"; "{}", rss_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

fn page_to_rss_item(page: &FeedPage, config: &SiteConfig) -> Option<rss::Item> {
    let pub_date = DateTimeUtc::parse(&page.date).map(DateTimeUtc::to_rfc2822)?;

    // Build full URL from base URL + permalink
    let base_url = config
        .site
        .info
        .url
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/');
    let link = format!("{}{}", base_url, page.permalink);

    let author = normalize_rss_author(page.author.as_ref(), config);

    // Convert summary JSON to HTML string using shared extractor
    let description = page.summary.clone();

    Some(
        ItemBuilder::default()
            .title(page.title.clone())
            .link(Some(link.clone()))
            .guid(GuidBuilder::default().permalink(true).value(link).build())
            .description(description)
            .pub_date(pub_date)
            .author(author)
            .build(),
    )
}

/// Normalize author field to RSS format: "email (Name)"
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
    let site_author = &config.site.info.author;
    if RE_VALID_AUTHOR.is_match(site_author) {
        return Some(site_author.clone());
    }

    // Combine email and author name
    Some(format!("{} ({})", config.site.info.email, site_author))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a config for testing
    fn make_config(author: &str, email: &str) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.site.info.author = author.to_string();
        config.site.info.email = email.to_string();
        config.site.info.url = Some("https://example.com".to_string());
        config
    }

    #[test]
    fn test_normalize_rss_author_valid_post() {
        let config = make_config("Site Author", "site@example.com");
        let author = "post@example.com (Post Author)".to_string();
        let result = normalize_rss_author(Some(&author), &config);
        assert_eq!(result, Some("post@example.com (Post Author)".to_string()));
    }

    #[test]
    fn test_normalize_rss_author_valid_site() {
        let config = make_config("site@example.com (Site Author)", "unused@example.com");
        let author = "Just a name".to_string();
        let result = normalize_rss_author(Some(&author), &config);
        assert_eq!(result, Some("site@example.com (Site Author)".to_string()));
    }

    #[test]
    fn test_normalize_rss_author_combined() {
        let config = make_config("Site Author", "site@example.com");
        let author = "Just a name".to_string();
        let result = normalize_rss_author(Some(&author), &config);
        assert_eq!(result, Some("site@example.com (Site Author)".to_string()));
    }

    #[test]
    fn test_normalize_rss_author_none() {
        let config = make_config("Site Author", "site@example.com");
        let result = normalize_rss_author(None, &config);
        assert_eq!(result, None);
    }

    #[test]
    fn test_page_to_rss_item_basic() {
        let config = make_config("Test Author", "test@example.com");
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some("A test summary".to_string()),
            author: None,
        };

        let item = page_to_rss_item(&page, &config).expect("should create item");
        assert_eq!(item.title(), Some("Test Post"));
        assert_eq!(item.link(), Some("https://example.com/test/"));
        assert_eq!(item.description(), Some("A test summary"));
    }

    #[test]
    fn test_page_to_rss_item_invalid_date() {
        let config = make_config("Test Author", "test@example.com");
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "invalid-date".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            author: None,
        };

        // Invalid date format should return None
        assert!(page_to_rss_item(&page, &config).is_none());
    }
}
