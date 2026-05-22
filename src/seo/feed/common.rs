//! Common utilities for feed generation.

use crate::{
    config::{FeedConfig, FeedFeature, SiteConfig},
    core::UrlPath,
    logger,
    page::{StoredPage, StoredPageMap},
    seo::extract::{extract, extract_text},
};
use anyhow::Result;
use std::{fs, path::Path};

/// A page validated for feed inclusion (requires title and date)
#[derive(Debug, Clone)]
pub struct FeedPage {
    pub title: String,
    pub date: String,
    pub permalink: String,
    pub summary: Option<FeedSummary>,
    pub feed_body: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FeedSummary {
    pub html: String,
    pub text: String,
}

pub struct FeedPages {
    pub pages: Vec<FeedPage>,
    pub excluded: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    Unchanged,
}

impl FeedPage {
    fn from_stored(page: &StoredPage) -> Option<Self> {
        Some(Self {
            title: page.meta.title.clone()?,
            date: page.meta.date.clone()?,
            permalink: page.permalink.to_string(),
            summary: page.meta.summary.as_ref().map(|summary| FeedSummary {
                html: extract(summary),
                text: extract_text(summary),
            }),
            feed_body: page.feed_body.clone(),
            author: page.meta.author.clone(),
        })
    }
}

/// Get all pages valid for feed inclusion (only pages with date)
pub fn collect_feed_pages(store: &StoredPageMap) -> FeedPages {
    let all_pages = store.get_pages();
    let total = all_pages.len();

    let pages: Vec<FeedPage> = all_pages.iter().filter_map(FeedPage::from_stored).collect();
    let excluded = total - pages.len();

    FeedPages { pages, excluded }
}

pub fn page_url(page: &FeedPage, config: &SiteConfig) -> String {
    config.canonical_url(&UrlPath::from_page(&page.permalink))
}

pub fn feed_url(config: &SiteConfig, feed: &FeedConfig) -> String {
    feed.url.canonical_url(config.site.info.url.as_deref())
}

pub fn summary_html(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<String> {
    page.summary
        .as_ref()
        .map(|summary| prepare_html(&summary.html, page, config, feed))
}

pub fn entry_body(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<String> {
    feed.has_feature(FeedFeature::FullText)
        .then_some(page.feed_body.as_deref())
        .flatten()
        .map(|html| prepare_html(html, page, config, feed))
}

pub fn entry_content(page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> Option<String> {
    entry_body(page, config, feed).or_else(|| summary_html(page, config, feed))
}

fn prepare_html(html: &str, page: &FeedPage, config: &SiteConfig, feed: &FeedConfig) -> String {
    super::html::prepare(
        html,
        &super::html::HtmlOptions {
            site_url: config.site.info.url.as_deref(),
            page_url: &page_url(page, config),
            no_script: feed.has_feature(FeedFeature::NoScript),
        },
    )
}

fn write_if_changed(path: &Path, content: &str) -> Result<WriteOutcome> {
    if let Ok(existing) = fs::read(path)
        && existing == content.as_bytes()
    {
        return Ok(WriteOutcome::Unchanged);
    }

    fs::write(path, content)?;
    Ok(WriteOutcome::Written)
}

pub fn write_feed(config: &SiteConfig, feed: &FeedConfig, content: &str) -> Result<WriteOutcome> {
    let output_path = feed.url.output_path(config.paths());

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let outcome = write_if_changed(&output_path, content)?;
    if outcome == WriteOutcome::Written {
        logger::log(
            "feed",
            format_args!(
                "{}: {}",
                feed.format.as_str(),
                output_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
        );
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{FeedPage, FeedSummary, WriteOutcome, write_if_changed};
    use crate::config::{FeedConfig, FeedFeature, FeedFormat, SiteConfig};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn write_if_changed_skips_identical_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("feed.xml");

        assert_eq!(
            write_if_changed(&path, "<rss/>").unwrap(),
            WriteOutcome::Written
        );
        assert_eq!(
            write_if_changed(&path, "<rss/>").unwrap(),
            WriteOutcome::Unchanged
        );
        assert_eq!(
            write_if_changed(&path, "<feed/>").unwrap(),
            WriteOutcome::Written
        );

        assert_eq!(fs::read_to_string(path).unwrap(), "<feed/>");
    }

    #[test]
    fn feed_html_uses_absolute_urls_and_preserves_scripts_by_default() {
        let mut config = SiteConfig::default();
        config.site.info.url = Some("https://example.com".to_string());
        let feed = FeedConfig {
            format: FeedFormat::Rss,
            url: "/feed.xml".into(),
            features: vec![FeedFeature::FullText],
        };
        let page = FeedPage {
            title: "Post".to_string(),
            date: "2026-05-12".to_string(),
            permalink: "/posts/中文/".to_string(),
            summary: Some(FeedSummary {
                html: r#"<a href="/about/">About</a>"#.to_string(),
                text: "About".to_string(),
            }),
            feed_body: Some(
                r##"<p><a href="#top">Top</a><img src="/img/中文.png"><script>fold()</script></p>"##
                    .to_string(),
            ),
            author: None,
        };

        let html = super::entry_content(&page, &config, &feed).unwrap();

        assert!(html.contains(r#"href="https://example.com/posts/%E4%B8%AD%E6%96%87/#top""#));
        assert!(html.contains(r#"src="https://example.com/img/%E4%B8%AD%E6%96%87.png""#));
        assert!(html.contains("<script>fold()</script>"));
    }

    #[test]
    fn no_script_removes_scripts_and_event_handlers() {
        let mut config = SiteConfig::default();
        config.site.info.url = Some("https://example.com".to_string());
        let feed = FeedConfig {
            format: FeedFormat::Rss,
            url: "/feed.xml".into(),
            features: vec![FeedFeature::FullText, FeedFeature::NoScript],
        };
        let page = FeedPage {
            title: "Post".to_string(),
            date: "2026-05-12".to_string(),
            permalink: "/posts/post/".to_string(),
            summary: None,
            feed_body: Some(
                r#"<button onclick="fold()">Fold</button><script>fold()</script>"#.to_string(),
            ),
            author: None,
        };

        let html = super::entry_content(&page, &config, &feed).unwrap();

        assert!(html.contains("<button>Fold</button>"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("<script"));
    }
}
