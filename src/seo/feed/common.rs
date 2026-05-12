//! Common utilities for feed generation.

use crate::{
    config::{FeedConfig, SiteConfig},
    log,
    page::{StoredPage, StoredPageMap},
    seo::extract::extract,
};
use anyhow::Result;
use std::{fs, path::Path};

/// A page validated for feed inclusion (requires title and date)
#[derive(Debug, Clone)]
pub struct FeedPage {
    pub title: String,
    pub date: String,
    pub permalink: String,
    pub summary: Option<String>,
    pub feed_body: Option<String>,
    pub author: Option<String>,
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
            summary: page.meta.summary.as_ref().map(extract),
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

fn write_if_changed(path: &Path, content: &str) -> Result<WriteOutcome> {
    if let Ok(existing) = fs::read(path)
        && existing == content.as_bytes()
    {
        return Ok(WriteOutcome::Unchanged);
    }

    fs::write(path, content)?;
    Ok(WriteOutcome::Written)
}

pub fn write_feed(config: &SiteConfig, feed: &FeedConfig, xml: &str) -> Result<WriteOutcome> {
    let output_path = config.paths().output_dir().join(&feed.path);

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let outcome = write_if_changed(&output_path, xml)?;
    if outcome == WriteOutcome::Written {
        log!(
            "feed";
            "{}: {}",
            feed.format.as_str(),
            output_path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{WriteOutcome, write_if_changed};
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
}
