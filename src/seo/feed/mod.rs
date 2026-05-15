//! Feed generation (RSS, Atom, JSON Feed).
//!
//! Generates syndication feeds from compiled page metadata:
//!
//! - **RSS 2.0**: Standard feed format
//! - **Atom 1.0**: Modern feed format
//! - **JSON Feed 1.1**: JSON feed format

use crate::config::{FeedConfig, FeedFormat, SiteConfig};
use crate::page::StoredPageMap;
use anyhow::Result;

pub mod atom;
mod common;
mod html;
pub mod json;
pub mod rss;

pub(crate) use common::feed_url;
use common::{FeedPage, WriteOutcome, collect_feed_pages, write_feed};

/// Build all configured feeds.
pub fn build_feed(config: &SiteConfig, store: &StoredPageMap) -> Result<()> {
    let outputs = config.site.seo.feed_outputs();
    if outputs.is_empty() {
        return Ok(());
    }

    let feed_pages = collect_feed_pages(store);
    let mut written_outputs = 0usize;

    for feed in outputs {
        let content = render_one(config, feed, &feed_pages.pages)?;
        if write_feed(config, feed, &content)? == WriteOutcome::Written {
            written_outputs += 1;
        }
    }

    if written_outputs > 0 && feed_pages.excluded > 0 {
        crate::log!(
            "feed";
            "excluded {} pages without date (only pages with date are included)",
            feed_pages.excluded
        );
    }

    Ok(())
}

fn render_one(config: &SiteConfig, feed: &FeedConfig, pages: &[FeedPage]) -> Result<String> {
    match feed.format {
        FeedFormat::Rss => rss::render(config, feed, pages),
        FeedFormat::Atom => atom::render(config, feed, pages),
        FeedFormat::Json => json::render(config, feed, pages),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FeedFeature;
    use crate::core::UrlPath;
    use crate::page::{PageMeta, StoredPageMap};
    use std::fs;
    use tempfile::TempDir;

    fn config_with_feeds(output: std::path::PathBuf) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.build.output = output;
        config.site.info.title = "Test Site".to_string();
        config.site.info.description = "Test feed".to_string();
        config.site.info.url = Some("https://example.com".to_string());
        config.site.seo.feeds = vec![
            FeedConfig {
                url: "/feed.xml".into(),
                format: FeedFormat::Rss,
                features: vec![],
            },
            FeedConfig {
                url: "/atom.xml".into(),
                format: FeedFormat::Atom,
                features: vec![],
            },
            FeedConfig {
                url: "/feed.json".into(),
                format: FeedFormat::Json,
                features: vec![],
            },
        ];
        config
    }

    fn store_with_page(feed_body: Option<String>) -> StoredPageMap {
        let store = StoredPageMap::new();
        store.insert_page_with_feed_body(
            UrlPath::from_page("/post 中文/"),
            PageMeta {
                title: Some("Post".to_string()),
                date: Some("2026-05-12".to_string()),
                summary: Some(serde_json::json!("Summary")),
                ..Default::default()
            },
            feed_body,
        );
        store
    }

    #[test]
    fn builds_all_configured_feed_outputs() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let config = config_with_feeds(output.clone());
        let store = store_with_page(None);

        build_feed(&config, &store).unwrap();

        let rss = fs::read_to_string(output.join("feed.xml")).unwrap();
        let atom = fs::read_to_string(output.join("atom.xml")).unwrap();
        let json = fs::read_to_string(output.join("feed.json")).unwrap();

        assert!(rss.contains("<rss"));
        assert!(rss.contains("<title>Post</title>"));
        assert!(atom.contains("<feed"));
        assert!(atom.contains("<title>Post</title>"));

        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["version"], "https://jsonfeed.org/version/1.1");
        assert_eq!(json["title"], "Test Site");
        assert_eq!(json["home_page_url"], "https://example.com/");
        assert_eq!(json["feed_url"], "https://example.com/feed.json");
        assert_eq!(
            json["items"][0]["id"],
            "https://example.com/post%20%E4%B8%AD%E6%96%87/"
        );
        assert_eq!(
            json["items"][0]["url"],
            "https://example.com/post%20%E4%B8%AD%E6%96%87/"
        );
        assert_eq!(json["items"][0]["title"], "Post");
        assert_eq!(json["items"][0]["date_published"], "2026-05-12T00:00:00Z");
        assert_eq!(json["items"][0]["summary"], "Summary");
        assert_eq!(json["items"][0]["content_html"], "Summary");
        assert!(json["items"][0]["content_text"].is_null());
    }

    #[test]
    fn skips_when_no_feed_outputs_are_configured() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let mut config = config_with_feeds(output.clone());
        config.site.seo.feeds.clear();
        let store = store_with_page(None);

        build_feed(&config, &store).unwrap();
        assert!(!output.exists());
    }

    #[test]
    fn full_text_feature_writes_full_entry_content() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let mut config = config_with_feeds(output.clone());
        config.site.seo.feeds[0].features = vec![FeedFeature::FullText];
        config.site.seo.feeds[1].features = vec![FeedFeature::FullText];
        config.site.seo.feeds[2].features = vec![FeedFeature::FullText];
        let store = store_with_page(Some("<article><p>Full text</p></article>".to_string()));

        build_feed(&config, &store).unwrap();

        let rss = fs::read_to_string(output.join("feed.xml")).unwrap();
        let atom = fs::read_to_string(output.join("atom.xml")).unwrap();
        let json = fs::read_to_string(output.join("feed.json")).unwrap();

        let rss = ::rss::Channel::read_from(rss.as_bytes()).unwrap();
        let rss_item = rss.items().first().unwrap();
        assert_eq!(rss_item.description(), Some("Summary"));
        assert_eq!(
            rss_item.content(),
            Some("<article><p>Full text</p></article>")
        );

        let atom = atom_syndication::Feed::read_from(atom.as_bytes()).unwrap();
        let atom_entry = atom.entries().first().unwrap();
        assert_eq!(
            atom_entry.summary().map(atom_syndication::Text::as_str),
            Some("Summary")
        );
        let atom_content = atom_entry.content().unwrap();
        assert_eq!(atom_content.content_type(), Some("html"));
        assert_eq!(
            atom_content.value(),
            Some("<article><p>Full text</p></article>")
        );

        let json: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json["items"][0]["summary"], "Summary");
        assert_eq!(
            json["items"][0]["content_html"],
            "<article><p>Full text</p></article>"
        );
    }

    #[test]
    fn prefixed_site_urls_are_canonicalized_once() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let mut config = config_with_feeds(output.clone());
        config.site.info.url = Some("https://example.com/docs/blog".to_string());
        config.build.path_prefix = "docs/blog".into();

        let store = StoredPageMap::new();
        store.insert_page_with_feed_body(
            UrlPath::from_page("/post/"),
            PageMeta {
                title: Some("Post".to_string()),
                date: Some("2026-05-12".to_string()),
                summary: Some(serde_json::json!("Summary")),
                ..Default::default()
            },
            None,
        );

        build_feed(&config, &store).unwrap();

        let json = fs::read_to_string(output.join("docs/blog/feed.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(json["home_page_url"], "https://example.com/docs/blog/");
        assert_eq!(json["feed_url"], "https://example.com/docs/blog/feed.json");
        assert_eq!(
            json["items"][0]["url"],
            "https://example.com/docs/blog/post/"
        );
    }
}
