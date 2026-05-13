//! RSS 2.0 feed generation.
//!
//! Generates RSS feeds from page metadata.

use super::common::{FeedPage, entry_body, page_url, summary_html};
use crate::{
    config::{FeedConfig, SiteConfig},
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
        let base_url = self.config.canonical_url(&UrlPath::from_page("/"));
        let items: Vec<_> = self
            .pages
            .iter()
            .filter_map(|page| page_to_rss_item(page, self.config, self.feed))
            .collect();

        let channel = ChannelBuilder::default()
            .title(&self.config.site.info.title)
            .link(base_url)
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

fn page_to_rss_item(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<rss::Item> {
    let pub_date = DateTimeUtc::parse(&page.date).map(DateTimeUtc::to_rfc2822)?;

    let link = page_url(page, config);

    let author = normalize_rss_author(page.author.as_ref(), config);

    let description = summary_html(page, config, feed);

    Some(
        ItemBuilder::default()
            .title(page.title.clone())
            .link(Some(link.clone()))
            .guid(GuidBuilder::default().permalink(true).value(link).build())
            .description(description)
            .content(entry_body(page, config, feed))
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
    use crate::config::{FeedFeature, FeedFormat};
    use crate::seo::feed::common::FeedSummary;

    // Helper to create a config for testing
    fn make_config(author: &str, email: &str) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.site.info.author = author.to_string();
        config.site.info.email = email.to_string();
        config.site.info.url = Some("https://example.com".to_string());
        config
    }

    fn feed_config(features: Vec<FeedFeature>) -> FeedConfig {
        FeedConfig {
            format: FeedFormat::Rss,
            path: "feed.xml".into(),
            features,
        }
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
            summary: Some(FeedSummary {
                html: "A test summary".to_string(),
                text: "A test summary".to_string(),
            }),
            feed_body: None,
            author: None,
        };

        let feed = feed_config(vec![]);
        let item = page_to_rss_item(&page, &config, &feed).expect("should create item");
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
            summary: Some(FeedSummary {
                html: "A test summary".to_string(),
                text: "A test summary".to_string(),
            }),
            feed_body: Some("<article><p>Full text</p></article>".to_string()),
            author: None,
        };

        let feed = feed_config(vec![FeedFeature::FullText]);
        let item = page_to_rss_item(&page, &config, &feed).expect("should create item");
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
        let feed = feed_config(vec![]);
        assert!(page_to_rss_item(&page, &config, &feed).is_none());
    }
}
