//! Feed generation (RSS, Atom).
//!
//! Generates syndication feeds from compiled page metadata:
//!
//! - **RSS 2.0**: Standard feed format (`rss.xml`)
//! - **Atom 1.0**: Modern feed format (`atom.xml`)

use crate::config::{FeedConfig, FeedFormat, SiteConfig};
use crate::page::StoredPageMap;
use anyhow::Result;

pub mod atom;
mod common;
pub mod rss;

/// Build all configured feeds.
pub fn build_feed(config: &SiteConfig, store: &StoredPageMap) -> Result<()> {
    for feed in config.site.seo.feed_outputs() {
        build_one(config, feed, store)?;
    }
    Ok(())
}

fn build_one(config: &SiteConfig, feed: &FeedConfig, store: &StoredPageMap) -> Result<()> {
    match feed.format {
        FeedFormat::Rss => rss::build_rss(config, feed, store),
        FeedFormat::Atom => atom::build_atom(config, feed, store),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            },
            FeedConfig {
                path: "atom.xml".into(),
                format: FeedFormat::Atom,
            },
        ];
        config
    }

    fn store_with_page() -> StoredPageMap {
        let store = StoredPageMap::new();
        store.insert_page(
            UrlPath::from_page("/post/"),
            PageMeta {
                title: Some("Post".to_string()),
                date: Some("2026-05-12".to_string()),
                summary: Some(serde_json::json!("Summary")),
                ..Default::default()
            },
        );
        store
    }

    #[test]
    fn builds_all_configured_feed_outputs() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("public");
        let config = config_with_feeds(output.clone());
        let store = store_with_page();

        build_feed(&config, &store).unwrap();

        let rss = fs::read_to_string(output.join("feed.xml")).unwrap();
        let atom = fs::read_to_string(output.join("atom.xml")).unwrap();

        assert!(rss.contains("<rss"));
        assert!(rss.contains("<title>Post</title>"));
        assert!(atom.contains("<feed"));
        assert!(atom.contains("<title>Post</title>"));
    }
}
