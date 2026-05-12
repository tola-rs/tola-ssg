//! RSS 2.0 feed generation.
//!
//! Generates RSS feeds from page metadata.

use super::common::FeedPage;
use crate::{
    config::{FeedConfig, FeedFeature, SiteConfig},
    core::UrlPath,
    utils::date::DateTimeUtc,
};
use anyhow::{Ok, Result, anyhow};
use regex::Regex;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder, validation::Validate};
use std::sync::LazyLock;

/// Render an RSS 2.0 feed.
pub(super) fn render(config: &SiteConfig, feed: &FeedConfig, pages: &[FeedPage]) -> Result<String> {
    RssFeed {
        config,
        feed,
        pages,
    }
    .to_xml()
}

struct RssFeed<'a> {
    config: &'a SiteConfig,
    feed: &'a FeedConfig,
    pages: &'a [FeedPage],
}

impl RssFeed<'_> {
    fn to_xml(&self) -> Result<String> {
        let items: Vec<_> = self
            .pages
            .iter()
            .filter_map(|page| {
                page_to_rss_item(
                    page,
                    &self.config,
                    self.feed.has_feature(FeedFeature::FullText),
                )
            })
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
}

fn page_to_rss_item(
    page: &FeedPage,
    config: &SiteConfig,
    include_full_text: bool,
) -> Option<rss::Item> {
    let pub_date = DateTimeUtc::parse(&page.date).map(DateTimeUtc::to_rfc2822)?;

    let permalink = UrlPath::from_page(&page.permalink);
    let link = permalink.canonical_url(config.site.info.url.as_deref());

    let author = normalize_rss_author(page.author.as_ref(), config);

    // Convert summary JSON to HTML string using shared extractor
    let description = page.summary.clone();

    Some(
        ItemBuilder::default()
            .title(page.title.clone())
            .link(Some(link.clone()))
            .guid(GuidBuilder::default().permalink(true).value(link).build())
            .description(description)
            .content(include_full_text.then(|| page.feed_body.clone()).flatten())
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
    fn test_normalize_rss_author_cases() {
        let config = make_config("Site Author", "site@example.com");
        let post_author = "post@example.com (Post Author)".to_string();
        assert_eq!(
            normalize_rss_author(Some(&post_author), &config),
            Some("post@example.com (Post Author)".to_string())
        );

        let site_valid = make_config("site@example.com (Site Author)", "unused@example.com");
        let plain_author = "Just a name".to_string();
        assert_eq!(
            normalize_rss_author(Some(&plain_author), &site_valid),
            Some("site@example.com (Site Author)".to_string())
        );

        assert_eq!(
            normalize_rss_author(Some(&plain_author), &config),
            Some("site@example.com (Site Author)".to_string())
        );

        assert_eq!(normalize_rss_author(None, &config), None);
    }

    #[test]
    fn test_page_to_rss_item_basic() {
        let config = make_config("Test Author", "test@example.com");
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some("A test summary".to_string()),
            feed_body: None,
            author: None,
        };

        let item = page_to_rss_item(&page, &config, false).expect("should create item");
        assert_eq!(item.title(), Some("Test Post"));
        assert_eq!(item.link(), Some("https://example.com/test/"));
        assert_eq!(item.description(), Some("A test summary"));
        assert_eq!(item.content(), None);
    }

    #[test]
    fn test_page_to_rss_item_full_text() {
        let config = make_config("Test Author", "test@example.com");
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "2024-01-15".to_string(),
            permalink: "/test/".to_string(),
            summary: Some("A test summary".to_string()),
            feed_body: Some("<article><p>Full text</p></article>".to_string()),
            author: None,
        };

        let item = page_to_rss_item(&page, &config, true).expect("should create item");
        assert_eq!(item.description(), Some("A test summary"));
        assert_eq!(item.content(), Some("<article><p>Full text</p></article>"));
    }

    #[test]
    fn test_page_to_rss_item_invalid_date() {
        let config = make_config("Test Author", "test@example.com");
        let page = FeedPage {
            title: "Test Post".to_string(),
            date: "invalid-date".to_string(),
            permalink: "/test/".to_string(),
            summary: None,
            feed_body: None,
            author: None,
        };

        // Invalid date format should return None
        assert!(page_to_rss_item(&page, &config, false).is_none());
    }
}
