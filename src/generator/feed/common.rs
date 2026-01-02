//! Common utilities for feed generation.

use crate::{
    generator::extract::extract,
    log,
    page::{STORED_PAGES, StoredPage},
};

/// A page validated for feed inclusion (requires title and date).
#[derive(Debug, Clone)]
pub struct FeedPage {
    pub title: String,
    pub date: String,
    pub permalink: String,
    pub summary: Option<String>,
    pub author: Option<String>,
}

impl FeedPage {
    fn from_stored(page: &StoredPage) -> Option<Self> {
        Some(Self {
            title: page.meta.title.clone()?,
            date: page.meta.date.clone()?,
            permalink: page.permalink.to_string(),
            summary: page.meta.summary.as_ref().map(extract),
            author: page.meta.author.clone(),
        })
    }
}

/// Get all pages valid for feed inclusion (only pages with date).
pub fn get_feed_pages() -> Vec<FeedPage> {
    let all_pages = STORED_PAGES.get_pages();
    let total = all_pages.len();

    let feed_pages: Vec<FeedPage> = all_pages.iter().filter_map(FeedPage::from_stored).collect();

    // Log excluded pages count (Zola-style strict filtering)
    let excluded = total - feed_pages.len();
    if excluded > 0 {
        log!("feed"; "excluded {} pages without date (only pages with date are included)", excluded);
    }

    feed_pages
}
