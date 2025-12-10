//! Global site data storage.
//!
//! Provides a thread-safe store for collecting and accessing page metadata
//! across the two-phase compilation process.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use parking_lot::RwLock;

use super::types::{PageData, TaggedPage, TagsIndex};

/// Compare two date strings for sorting (newest first).
///
/// - Items with dates come before items without dates
/// - Items with same date are sorted by title
fn compare_by_date<T: AsRef<str>>(a_date: &Option<String>, b_date: &Option<String>, a_title: T, b_title: T) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (b_date, a_date) {
        (Some(date_b), Some(date_a)) => date_a.cmp(date_b).reverse(),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a_title.as_ref().cmp(b_title.as_ref()),
    }
}

/// Global site data store, accessible from anywhere in the compilation process.
///
/// This is initialized lazily and can be reset between builds (e.g., in watch mode).
pub static GLOBAL_SITE_DATA: LazyLock<SiteDataStore> = LazyLock::new(SiteDataStore::new);

/// Thread-safe storage for site-wide data.
///
/// # Thread Safety
///
/// Uses `RwLock` to allow:
/// - Multiple concurrent reads (during Phase 2 compilation)
/// - Exclusive writes (during Phase 1 metadata collection)
#[derive(Debug, Default)]
pub struct SiteDataStore {
    pages: RwLock<BTreeMap<String, PageData>>,
}

impl SiteDataStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all stored data.
    ///
    /// Call this at the start of each build to ensure fresh data.
    pub fn clear(&self) {
        self.pages.write().clear();
    }

    /// Insert or update a page's data.
    ///
    /// The URL is used as the key to avoid duplicates.
    pub fn insert_page(&self, page: PageData) {
        self.pages.write().insert(page.url.clone(), page);
    }

    /// Get all pages as a sorted vector.
    ///
    /// Pages are sorted by date (newest first), then by title.
    /// Draft pages are excluded from the output.
    pub fn get_pages(&self) -> Vec<PageData> {
        let pages = self.pages.read();
        let mut result: Vec<_> = pages.values()
            .filter(|p| !p.draft)
            .cloned()
            .collect();
        result.sort_by(|a, b| compare_by_date(&a.date, &b.date, &a.title, &b.title));
        result
    }

    /// Build the tags index from stored pages.
    ///
    /// Returns a map from tag name to list of pages with that tag.
    pub fn get_tags_index(&self) -> TagsIndex {
        let pages = self.pages.read();
        let mut tags: TagsIndex = BTreeMap::new();

        for page in pages.values() {
            // Skip drafts from tag index
            if page.draft {
                continue;
            }

            for tag in &page.tags {
                tags.entry(tag.clone()).or_default().push(TaggedPage {
                    url: page.url.clone(),
                    title: page.title.clone(),
                    date: page.date.clone(),
                });
            }
        }

        // Sort pages within each tag by date (newest first)
        for pages in tags.values_mut() {
            pages.sort_by(|a, b| compare_by_date(&a.date, &b.date, &a.title, &b.title));
        }

        tags
    }

    /// Serialize pages to JSON.
    pub fn pages_to_json(&self) -> String {
        let pages = self.get_pages();
        serde_json::to_string_pretty(&pages).unwrap_or_else(|_| "[]".to_string())
    }

    /// Serialize tags index to JSON.
    pub fn tags_to_json(&self) -> String {
        let tags = self.get_tags_index();
        serde_json::to_string_pretty(&tags).unwrap_or_else(|_| "{}".to_string())
    }

    /// Check if the store has any data.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.pages.read().is_empty()
    }

    /// Get the number of pages.
    #[allow(dead_code)]
    pub fn page_count(&self) -> usize {
        self.pages.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get_pages() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/posts/first/".to_string(),
            title: "First Post".to_string(),
            summary: None,
            date: Some("2024-01-15".to_string()),
            update: None,
            author: None,
            tags: vec!["rust".to_string()],
            draft: false,
        });

        store.insert_page(PageData {
            url: "/posts/second/".to_string(),
            title: "Second Post".to_string(),
            summary: None,
            date: Some("2024-01-20".to_string()),
            update: None,
            author: None,
            tags: vec!["rust".to_string(), "web".to_string()],
            draft: false,
        });

        let pages = store.get_pages();
        assert_eq!(pages.len(), 2);
        // Newest first
        assert_eq!(pages[0].title, "Second Post");
        assert_eq!(pages[1].title, "First Post");
    }

    #[test]
    fn test_tags_index() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/a/".to_string(),
            title: "A".to_string(),
            summary: None,
            date: Some("2024-01-10".to_string()),
            update: None,
            author: None,
            tags: vec!["rust".to_string()],
            draft: false,
        });

        store.insert_page(PageData {
            url: "/b/".to_string(),
            title: "B".to_string(),
            summary: None,
            date: Some("2024-01-20".to_string()),
            update: None,
            author: None,
            tags: vec!["rust".to_string(), "web".to_string()],
            draft: false,
        });

        let tags = store.get_tags_index();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags["rust"].len(), 2);
        assert_eq!(tags["web"].len(), 1);

        // Check sort order (newest first)
        assert_eq!(tags["rust"][0].title, "B");
        assert_eq!(tags["rust"][1].title, "A");
    }

    #[test]
    fn test_draft_excluded_from_tags() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/draft/".to_string(),
            title: "Draft".to_string(),
            summary: None,
            date: None,
            update: None,
            author: None,
            tags: vec!["test".to_string()],
            draft: true,
        });

        let tags = store.get_tags_index();
        assert!(tags.is_empty());
    }

    #[test]
    fn test_draft_excluded_from_pages() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/published/".to_string(),
            title: "Published".to_string(),
            summary: None,
            date: Some("2024-01-15".to_string()),
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        store.insert_page(PageData {
            url: "/draft/".to_string(),
            title: "Draft".to_string(),
            summary: None,
            date: Some("2024-01-20".to_string()),
            update: None,
            author: None,
            tags: vec![],
            draft: true,
        });

        let pages = store.get_pages();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].title, "Published");
    }

    #[test]
    fn test_sort_without_dates() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/b/".to_string(),
            title: "Beta".to_string(),
            summary: None,
            date: None,
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        store.insert_page(PageData {
            url: "/a/".to_string(),
            title: "Alpha".to_string(),
            summary: None,
            date: None,
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        let pages = store.get_pages();
        assert_eq!(pages.len(), 2);
        // Alphabetical by title when no dates
        assert_eq!(pages[0].title, "Alpha");
        assert_eq!(pages[1].title, "Beta");
    }

    #[test]
    fn test_sort_mixed_dates() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/no-date/".to_string(),
            title: "No Date".to_string(),
            summary: None,
            date: None,
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        store.insert_page(PageData {
            url: "/has-date/".to_string(),
            title: "Has Date".to_string(),
            summary: None,
            date: Some("2024-01-15".to_string()),
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        let pages = store.get_pages();
        assert_eq!(pages.len(), 2);
        // Pages with dates come first
        assert_eq!(pages[0].title, "Has Date");
        assert_eq!(pages[1].title, "No Date");
    }

    #[test]
    fn test_clear() {
        let store = SiteDataStore::new();

        store.insert_page(PageData {
            url: "/test/".to_string(),
            title: "Test".to_string(),
            summary: None,
            date: None,
            update: None,
            author: None,
            tags: vec![],
            draft: false,
        });

        assert!(!store.is_empty());
        store.clear();
        assert!(store.is_empty());
    }
}
