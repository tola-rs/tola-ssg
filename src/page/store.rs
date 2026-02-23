//! Global page storage for virtual package injection and RSS/sitemap.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHasher};
use serde::Serialize;

use super::PageMeta;
use super::links::PAGE_LINKS;
use crate::compiler::page::ScannedHeading;
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::package::{Phase, TolaPackage};

/// Global page data store
pub static STORED_PAGES: LazyLock<StoredPageMap> = LazyLock::new(StoredPageMap::new);

/// A page entry stored in the global page data
///
/// Combines the computed permalink with page metadata
/// Serializes with `permalink` as top-level field and PageMeta flattened
#[derive(Debug, Clone, Serialize)]
pub struct StoredPage {
    /// The page's permalink (URL path).
    pub permalink: UrlPath,
    /// Page metadata from `<tola-meta>` (flattened in JSON output).
    #[serde(flatten)]
    pub meta: PageMeta,
}

impl StoredPage {
    pub fn new(permalink: UrlPath, meta: PageMeta) -> Self {
        Self { permalink, meta }
    }

    /// Check if this page is a draft.
    #[inline]
    pub fn is_draft(&self) -> bool {
        self.meta.draft
    }

    /// Get title, falling back to permalink if not set.
    pub fn title(&self) -> &str {
        self.meta
            .title
            .as_deref()
            .unwrap_or_else(|| self.permalink.as_str())
    }
}

/// Thread-safe storage for site-wide page data
///
/// Maps permalink (`UrlPath`) to `StoredPage`
/// Also stores page headings separately for @tola/current injection
#[derive(Debug, Default)]
pub struct StoredPageMap {
    /// Pages keyed by permalink.
    pages: RwLock<BTreeMap<UrlPath, StoredPage>>,
    /// Page headings keyed by permalink (not serialized to @tola/pages).
    headings: RwLock<FxHashMap<UrlPath, Vec<ScannedHeading>>>,
    /// Source file path to permalink mapping (for @tola/current lookup).
    source_to_url: RwLock<FxHashMap<PathBuf, UrlPath>>,
}

impl StoredPageMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.pages.write().clear();
        self.headings.write().clear();
        self.source_to_url.write().clear();
    }

    /// Insert or update a page.
    pub fn insert_page(&self, permalink: UrlPath, meta: PageMeta) {
        self.pages
            .write()
            .insert(permalink.clone(), StoredPage::new(permalink, meta));
    }

    /// Remove a page by permalink.
    pub fn remove_page(&self, permalink: &UrlPath) {
        self.pages.write().remove(permalink);
        self.headings.write().remove(permalink);
    }

    /// Insert headings for a page.
    pub fn insert_headings(&self, permalink: UrlPath, headings: Vec<ScannedHeading>) {
        if !headings.is_empty() {
            self.headings.write().insert(permalink, headings);
        }
    }

    /// Insert source file path to permalink mapping.
    pub fn insert_source_mapping(&self, source: PathBuf, permalink: UrlPath) {
        self.source_to_url.write().insert(source, permalink);
    }

    /// Get permalink by source file path.
    pub fn get_permalink_by_source(&self, source: &Path) -> Option<UrlPath> {
        self.source_to_url.read().get(source).cloned()
    }

    /// Get headings for a page.
    pub fn get_headings(&self, url: &UrlPath) -> Vec<ScannedHeading> {
        self.headings.read().get(url).cloned().unwrap_or_default()
    }

    /// Get a hash of current pages state (for change detection).
    ///
    /// Hashes the entire PageMeta (including user-defined `extra` fields)
    /// to ensure hot reload detects any metadata change.
    pub fn pages_hash(&self) -> u64 {
        let pages = self.pages.read();
        let mut hasher = FxHasher::default();
        for (url, page) in pages.iter() {
            url.hash(&mut hasher);
            // Hash entire PageMeta as JSON to catch all fields including `extra`
            if let Ok(json) = serde_json::to_string(&page.meta) {
                json.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Get all non-draft pages sorted by date (newest first).
    pub fn get_pages(&self) -> Vec<StoredPage> {
        let pages = self.pages.read();
        let mut result: Vec<_> = pages.values().filter(|p| !p.is_draft()).cloned().collect();
        result.sort_by(|a, b| {
            // Sort by date descending, then by title
            match (&b.meta.date, &a.meta.date) {
                (Some(date_b), Some(date_a)) => date_a.cmp(date_b).reverse(),
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (None, None) => a.title().cmp(b.title()),
            }
        });
        result
    }

    /// Get pages as JSON Value for injection via sys.inputs.
    ///
    /// Serializes to format expected by Typst templates:
    /// ```json
    /// [{ "permalink": "/path/", "title": "...", "summary": ..., ... }]
    /// ```
    pub fn pages_to_json_value(&self) -> serde_json::Value {
        let pages = self.get_pages();
        serde_json::to_value(&pages).unwrap_or(serde_json::Value::Array(vec![]))
    }

    /// Build `sys.inputs` with site config and pages data.
    ///
    /// Used by both build (iterative pages) and hot reload to inject
    /// data into Typst compilation via virtual packages:
    /// - `@tola/site` - Site configuration from tola.toml [site]
    /// - `@tola/pages` - Page metadata
    /// - Phase set to "compile" to indicate compile phase
    pub fn build_inputs(&self, config: &SiteConfig) -> anyhow::Result<typst_batch::Inputs> {
        let pages_json = self.pages_to_json_value();
        let site_info_json = serde_json::to_value(&config.site.info)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let mut combined = serde_json::Map::new();
        combined.insert(TolaPackage::Site.input_key(), site_info_json);
        combined.insert(TolaPackage::Pages.input_key(), pages_json);
        combined.insert(
            Phase::input_key().to_string(),
            serde_json::json!(Phase::Visible.as_str()),
        );
        // Inject format="html" for templates to detect HTML output.
        //
        // Why not just use `context { target() }`?
        // - target() requires a context block and is only evaluated during Layout phase
        // - Scan phase (Eval-only) cannot evaluate context blocks, so target() won't work
        // - Image show rules need to work during scan to extract image paths for validation
        //
        // Templates should use:
        // - `is-html` (sys.inputs.format) for image show rules (works during scan)
        // - `target() == "html"` for math show rules with html.frame() (avoids paged warnings)
        combined.insert("format".to_string(), serde_json::json!("html"));

        typst_batch::Inputs::from_json_with_content(
            &serde_json::Value::Object(combined),
            config.get_root(),
        )
        .map_err(|e| anyhow::anyhow!("failed to build site inputs: {}", e))
    }

    /// Build current page context for `@tola/current` virtual package.
    ///
    /// Returns JSON with `__tola_current` key for injection into `sys.inputs`.
    pub fn build_current_context(&self, url: &UrlPath) -> serde_json::Value {
        use crate::package::TolaPackage;

        let parent = url.parent().map(|p| p.as_str().to_string());
        let pages = self.pages.read();

        // Get link relationships as full page objects
        let links_to_urls = PAGE_LINKS.links_to(url);
        let linked_by_urls = PAGE_LINKS.linked_by(url);

        let links_to: Vec<&StoredPage> =
            links_to_urls.iter().filter_map(|u| pages.get(u)).collect();
        let linked_by: Vec<&StoredPage> =
            linked_by_urls.iter().filter_map(|u| pages.get(u)).collect();

        // Get headings for this page
        let headings = self.get_headings(url);

        // Wrap in __tola_current key for sys.inputs injection
        serde_json::json!({
            TolaPackage::Current.input_key(): {
                "path": url.as_str(),
                "parent": parent,
                "links_to": links_to,
                "linked_by": linked_by,
                "headings": headings,
            }
        })
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.pages.read().is_empty()
    }

    #[allow(dead_code)]
    pub fn page_count(&self) -> usize {
        self.pages.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_page(url: &str, title: &str, date: Option<&str>, draft: bool) -> (UrlPath, PageMeta) {
        (
            UrlPath::from_page(url),
            PageMeta {
                title: Some(title.to_string()),
                date: date.map(|s| s.to_string()),
                draft,
                ..Default::default()
            },
        )
    }

    #[test]
    fn test_insert_and_get_pages() {
        let store = StoredPageMap::new();
        let (url_a, meta_a) = make_page("/a/", "A", Some("2024-01-10"), false);
        let (url_b, meta_b) = make_page("/b/", "B", Some("2024-01-20"), false);
        store.insert_page(url_a, meta_a);
        store.insert_page(url_b, meta_b);

        let pages = store.get_pages();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].title(), "B"); // Newest first
        assert_eq!(pages[1].title(), "A");
    }

    #[test]
    fn test_draft_excluded() {
        let store = StoredPageMap::new();
        let (url_pub, meta_pub) = make_page("/pub/", "Published", Some("2024-01-15"), false);
        let (url_draft, meta_draft) = make_page("/draft/", "Draft", Some("2024-01-20"), true);
        store.insert_page(url_pub, meta_pub);
        store.insert_page(url_draft, meta_draft);

        let pages = store.get_pages();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].title(), "Published");
    }

    #[test]
    fn test_clear() {
        let store = StoredPageMap::new();
        let (url, meta) = make_page("/test/", "Test", None, false);
        store.insert_page(url, meta);

        assert!(!store.is_empty());
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn test_json_serialization() {
        let store = StoredPageMap::new();
        let (url, meta) = make_page("/hello/", "Hello World", Some("2024-01-15"), false);
        store.insert_page(url, meta);

        let json = store.pages_to_json_value();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);

        let page = &arr[0];
        assert_eq!(page["permalink"], "/hello/");
        assert_eq!(page["title"], "Hello World");
        assert_eq!(page["date"], "2024-01-15");
    }

    #[test]
    fn test_title_fallback_to_permalink() {
        let store = StoredPageMap::new();
        let url = UrlPath::from_page("/no-title/");
        let meta = PageMeta::default(); // No title
        store.insert_page(url, meta);

        let pages = store.get_pages();
        assert_eq!(pages[0].title(), "/no-title/");
    }

    #[test]
    fn test_extra_fields_serialized() {
        use super::super::JsonMap;

        let store = StoredPageMap::new();
        let url = UrlPath::from_page("/custom/");
        let mut extra = JsonMap::new();
        extra.insert(
            "custom_field".to_string(),
            serde_json::json!("custom_value"),
        );
        extra.insert("number".to_string(), serde_json::json!(42));

        let meta = PageMeta {
            title: Some("Custom".to_string()),
            extra,
            ..Default::default()
        };
        store.insert_page(url, meta);

        let json = store.pages_to_json_value();
        let page = &json.as_array().unwrap()[0];

        // User-defined fields should be present
        assert_eq!(page["custom_field"], "custom_value");
        assert_eq!(page["number"], 42);
        assert_eq!(page["permalink"], "/custom/");
    }
}
