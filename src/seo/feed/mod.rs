//! Feed generation (RSS, Atom).
//!
//! Generates syndication feeds from compiled page metadata:
//!
//! - **RSS 2.0**: Standard feed format
//! - **Atom 1.0**: Modern feed format

use crate::config::{FeedConfig, FeedFormat, SiteConfig};
use crate::page::StoredPageMap;
use anyhow::Result;

pub mod atom;
mod common;
pub mod rss;

use common::{FeedPage, get_feed_pages};

/// Build all configured feeds.
pub fn build_feed(config: &SiteConfig, store: &StoredPageMap) -> Result<()> {
    let pages = get_feed_pages(store);

    for feed in config.site.seo.feed_outputs() {
        build_one(config, feed, &pages)?;
    }
    Ok(())
}

fn build_one(config: &SiteConfig, feed: &FeedConfig, pages: &[FeedPage]) -> Result<()> {
    match feed.format {
        FeedFormat::Rss => rss::build_rss(config, feed, pages),
        FeedFormat::Atom => atom::build_atom(config, feed, pages),
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
                path: "feed.xml".into(),
                format: FeedFormat::Rss,
                features: vec![],
            },
            FeedConfig {
                path: "atom.xml".into(),
                format: FeedFormat::Atom,
                features: vec![],
            },
        ];
        config
    }

    fn store_with_page(feed_body: Option<String>) -> StoredPageMap {
        let store = StoredPageMap::new();
        store.insert_page_with_feed_body(
            UrlPath::from_page("/post/"),
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

        assert!(rss.contains("<rss"));
        assert!(rss.contains("<title>Post</title>"));
        assert!(atom.contains("<feed"));
        assert!(atom.contains("<title>Post</title>"));
    }

    #[test]
    fn full_text_feature_writes_full_entry_content() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let mut config = config_with_feeds(output.clone());
        config.site.seo.feeds[0].features = vec![FeedFeature::FullText];
        config.site.seo.feeds[1].features = vec![FeedFeature::FullText];
        let store = store_with_page(Some("<article><p>Full text</p></article>".to_string()));

        build_feed(&config, &store).unwrap();

        let rss = fs::read_to_string(output.join("feed.xml")).unwrap();
        let atom = fs::read_to_string(output.join("atom.xml")).unwrap();

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
    }
}
