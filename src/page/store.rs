//! Global page storage for virtual package injection and RSS/sitemap.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHasher};
use serde::Serialize;

use super::links::PAGE_LINKS;
use super::{CompiledPage, PageMeta};
use crate::compiler::page::ScannedHeading;
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::package::build_visible_inputs;
use crate::utils::path::normalize_path;

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

/// Controls whether stale backlink graph entries are cleared when permalink changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLinkPolicy {
    /// Keep existing link graph entries for old permalink.
    Keep,
    /// Clear link graph entries for old permalink.
    Clear,
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

    /// Remove a page by its source file path.
    ///
    /// Cleans up pages, headings, and source_to_url in one operation.
    pub fn remove_by_source(&self, source: &Path) {
        let normalized = normalize_path(source);
        let permalink = {
            let mut source_to_url = self.source_to_url.write();
            let removed = source_to_url
                .remove(&normalized)
                .or_else(|| source_to_url.remove(source));
            if source != normalized.as_path() {
                source_to_url.remove(source);
            }
            removed
        };

        if let Some(permalink) = permalink {
            self.pages.write().remove(&permalink);
            self.headings.write().remove(&permalink);
        }
    }

    /// Insert headings for a page.
    pub fn insert_headings(&self, permalink: UrlPath, headings: Vec<ScannedHeading>) {
        if !headings.is_empty() {
            self.headings.write().insert(permalink, headings);
        }
    }

    /// Insert source file path to permalink mapping.
    pub fn insert_source_mapping(&self, source: PathBuf, permalink: UrlPath) {
        let normalized = normalize_path(&source);
        let mut source_to_url = self.source_to_url.write();
        source_to_url.insert(normalized.clone(), permalink);
        if source != normalized {
            source_to_url.remove(&source);
        }
    }

    /// Get permalink by source file path.
    pub fn get_permalink_by_source(&self, source: &Path) -> Option<UrlPath> {
        let normalized = normalize_path(source);
        let source_to_url = self.source_to_url.read();
        source_to_url
            .get(normalized.as_path())
            .cloned()
            .or_else(|| source_to_url.get(source).cloned())
    }

    /// Keep source->permalink mapping consistent and optionally clear stale backlink data.
    ///
    /// If an existing mapping points to a different permalink, the old stored page is removed.
    pub fn sync_source_permalink(
        &self,
        source: &Path,
        new_permalink: UrlPath,
        stale_link_policy: StaleLinkPolicy,
    ) {
        if let Some(old_permalink) = self.get_permalink_by_source(source)
            && old_permalink != new_permalink
        {
            self.remove_page(&old_permalink);
            if matches!(stale_link_policy, StaleLinkPolicy::Clear) {
                PAGE_LINKS.record(&old_permalink, vec![]);
            }
        }
        self.insert_source_mapping(source.to_path_buf(), new_permalink);
    }

    /// Apply parsed metadata to a source file and update page storage in one step.
    ///
    /// Returns the resolved permalink on success. Returns `None` when route
    /// resolution fails (e.g. source path is outside content directory).
    pub fn apply_meta_for_source(
        &self,
        source: &Path,
        meta: PageMeta,
        config: &SiteConfig,
        stale_link_policy: StaleLinkPolicy,
    ) -> Option<UrlPath> {
        let compiled =
            CompiledPage::from_paths_with_meta(source, config, Some(meta.clone())).ok()?;

        let permalink = compiled.route.permalink;
        self.sync_source_permalink(source, permalink.clone(), stale_link_policy);
        self.insert_page(permalink.clone(), meta);
        Some(permalink)
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

    /// Get all pages (including drafts) sorted by date (newest first).
    pub fn get_pages_with_drafts(&self) -> Vec<StoredPage> {
        let pages = self.pages.read();
        let mut result: Vec<_> = pages.values().cloned().collect();
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

    /// Get all non-draft pages sorted by date (newest first).
    pub fn get_pages(&self) -> Vec<StoredPage> {
        self.get_pages_with_drafts()
            .into_iter()
            .filter(|p| !p.is_draft())
            .collect()
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

    /// Get all pages (including drafts) as JSON Value for injection via sys.inputs.
    pub fn pages_to_json_value_with_drafts(&self) -> serde_json::Value {
        let pages = self.get_pages_with_drafts();
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
        build_visible_inputs(config, self)
    }

    /// Build current page context for `@tola/current` virtual package.
    ///
    /// Returns JSON with `__tola_current` key for injection into `sys.inputs`.
    ///
    /// # Arguments
    /// * `url` - The page's permalink (URL path)
    /// * `path` - Optional source file path relative to content directory
    pub fn build_current_context(&self, url: &UrlPath, path: Option<&str>) -> serde_json::Value {
        use crate::package::TolaPackage;

        let parent = url.parent().map(|p| p.as_str().to_string());
        let filename = path.and_then(|s| {
            Path::new(s)
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        });
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
                "permalink": url.as_str(),
                "parent-permalink": parent,
                "path": path,
                "filename": filename,
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
    use std::fs;
    use tempfile::TempDir;

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
    fn test_get_pages_with_drafts_includes_drafts() {
        let store = StoredPageMap::new();
        let (url_pub, meta_pub) = make_page("/pub/", "Published", Some("2024-01-15"), false);
        let (url_draft, meta_draft) = make_page("/draft/", "Draft", Some("2024-01-20"), true);
        store.insert_page(url_pub, meta_pub);
        store.insert_page(url_draft, meta_draft);

        let pages = store.get_pages_with_drafts();
        assert_eq!(pages.len(), 2);
        assert!(
            pages
                .iter()
                .any(|p| p.permalink == UrlPath::from_page("/pub/"))
        );
        assert!(
            pages
                .iter()
                .any(|p| p.permalink == UrlPath::from_page("/draft/"))
        );
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
    fn test_json_with_drafts_serialization() {
        let store = StoredPageMap::new();
        let (url_pub, meta_pub) = make_page("/pub/", "Published", Some("2024-01-15"), false);
        let (url_draft, meta_draft) = make_page("/draft/", "Draft", Some("2024-01-20"), true);
        store.insert_page(url_pub, meta_pub);
        store.insert_page(url_draft, meta_draft);

        let json = store.pages_to_json_value_with_drafts();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().any(|p| p["permalink"] == "/pub/"));
        assert!(arr.iter().any(|p| p["permalink"] == "/draft/"));
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

    #[test]
    fn test_insert_and_lookup_source_mapping_normalizes_path() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file = content_dir.join("post.typ");
        fs::write(&file, "= Hello").unwrap();

        let alias = content_dir.join(".").join("post.typ");
        let permalink = UrlPath::from_page("/post/");
        let store = StoredPageMap::new();

        store.insert_source_mapping(alias.clone(), permalink.clone());

        assert_eq!(
            store.get_permalink_by_source(&file),
            Some(permalink.clone())
        );
        assert_eq!(store.get_permalink_by_source(&alias), Some(permalink));
    }

    #[test]
    fn test_apply_meta_for_source_removes_stale_permalink() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let file = content_dir.join("sample.typ");
        fs::write(&file, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let store = StoredPageMap::new();
        let old_permalink = UrlPath::from_page("/sample/");
        store.insert_page(
            old_permalink.clone(),
            PageMeta {
                title: Some("Old".to_string()),
                ..Default::default()
            },
        );
        store.insert_source_mapping(file.clone(), old_permalink.clone());

        let meta = PageMeta {
            title: Some("New".to_string()),
            permalink: Some("/custom/sample/".to_string()),
            ..Default::default()
        };

        let new_permalink = store
            .apply_meta_for_source(&file, meta, &config, StaleLinkPolicy::Keep)
            .expect("meta apply should succeed");

        assert_eq!(new_permalink, UrlPath::from_page("/custom/sample/"));
        assert_eq!(
            store.get_permalink_by_source(&file),
            Some(UrlPath::from_page("/custom/sample/"))
        );

        let pages = store.get_pages_with_drafts();
        assert!(
            pages
                .iter()
                .any(|p| p.permalink == UrlPath::from_page("/custom/sample/"))
        );
        assert!(!pages.iter().any(|p| p.permalink == old_permalink));
    }
}
