//! Compiled page and page collection types.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Result, anyhow};

use crate::asset::url_from_output_path;
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::page::PageMeta;
use crate::utils::path::slug::slugify_path;

use super::PageRoute;

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert days since Unix epoch to (year, month, day).
///
/// Uses a simplified leap year calculation that's accurate for dates
/// from 1970 to ~2100.
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Days from year 0 to 1970-01-01 (approximate, but works for our range)
    const DAYS_TO_1970: i64 = 719_468;

    let z = days + DAYS_TO_1970;
    let era = z.div_euclid(146_097); // 400-year cycles
    let doe = z.rem_euclid(146_097) as u32; // day of era [0, 146096]

    // Year of era [0, 399]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]

    // Month calculation (March = 0)
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y, m, d)
}

// ============================================================================
// CompiledPage
// ============================================================================

/// Primary metadata structure for a compiled content page.
///
/// Contains all path and URL information needed by build, rss and sitemap.
/// This is the **single source of truth** for page paths and content metadata.
///
/// # Fields
///
/// | Field | Example | Used By |
/// |-------|---------|---------|
/// | `route.source` | `content/posts/hello.typ` | build, rss query |
/// | `route.output_file` | `public/posts/hello/index.html` | build output |
/// | `route.permalink` | `/posts/hello/` | URL construction |
/// | `lastmod` | `SystemTime` | sitemap |
/// | `content_meta` | `PageMeta` | rss (title/summary/date) |
/// | `compiled_html` | `Vec<u8>` | Lib mode pre-compiled HTML |
///
/// Note: Previously named `PageMeta`. Renamed to avoid confusion with `page::PageMeta`.
#[derive(Debug, Clone)]
pub struct CompiledPage {
    /// Route information (source → output mapping)
    pub route: PageRoute,
    /// Last modification time of the HTML file
    pub lastmod: Option<SystemTime>,
    /// Content metadata from `<tola-meta>` (None if not present)
    pub content_meta: Option<PageMeta>,
    /// Pre-compiled HTML content (Lib mode only, None for CLI mode)
    pub compiled_html: Option<Vec<u8>>,
}

impl CompiledPage {
    /// Create `CompiledPage` from a source content file path without querying metadata.
    ///
    /// This is the lightweight version that only computes paths.
    /// Use `with_content` to set the content metadata later.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - File is not in content directory
    /// - File is not a supported content type (.typ, .md)
    pub fn from_paths(source: impl AsRef<Path>, config: &SiteConfig) -> Result<Self> {
        use crate::core::ContentKind;

        // Canonicalize source path to ensure consistency with content_dir
        // This fixes path mismatches like /var vs /private/var on macOS
        let source = crate::utils::path::normalize_path(source.as_ref());

        // Validate content type
        ContentKind::from_path(&source)
            .ok_or_else(|| anyhow!("Unsupported content type: {}", source.display()))?;

        // Also normalize content_dir for consistent comparison
        let content_dir = crate::utils::path::normalize_path(&config.build.content);
        let paths = config.paths();
        let output_root = paths.output_dir();
        let base_url = config.site
            .info
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');

        // Source directory
        let source_dir = source.parent().unwrap_or(Path::new("")).to_path_buf();

        // Check if this is an index file
        let is_index = source.file_stem().map(|s| s == "index").unwrap_or(false);

        // Strip content dir and extension
        let rel_path = source
            .strip_prefix(&content_dir)
            .map_err(|_| anyhow!("File is not in content directory: {}", source.display()))?;

        // Check if this is the 404 page (compare source path with config.build.not_found)
        // not_found is relative to site root, so we compare with source relative to root
        let is_404 = config
            .build
            .not_found
            .as_ref()
            .is_some_and(|nf| config.root_relative(&source) == *nf);

        // Use file_stem to handle any supported extension (.typ, .md, etc.)
        let relative = rel_path
            .with_extension("")
            .to_str()
            .ok_or_else(|| anyhow!("Invalid path encoding"))?
            .to_owned();

        let is_root_index = relative == "index";

        // Compute HTML output path
        // 404 page outputs as 404.html (not 404/index.html)
        // Root index outputs as index.html
        // Other pages output as {slug}/index.html
        let output_file = if is_404 {
            output_root.join("404.html")
        } else if is_root_index {
            output_root.join("index.html")
        } else {
            let slugified_relative = slugify_path(Path::new(&relative), &config.build.slug);
            output_root.join(slugified_relative).join("index.html")
        };

        // Output directory
        let output_dir = output_file.parent().unwrap_or(Path::new("")).to_path_buf();

        // Compute colocated assets directory
        // For non-index files: look for a directory with the same name as the file (without extension)
        // For index files: the source directory itself contains colocated assets
        let colocated_dir = if is_index {
            // For index.typ, colocated assets are in the same directory
            Some(source_dir.clone())
        } else {
            // For hello.typ, look for hello/ directory
            let potential_dir = source.with_extension("");
            if potential_dir.is_dir() {
                Some(potential_dir)
            } else {
                None
            }
        };

        // Compute URL path from the final HTML path to ensure consistency
        let full_path_url = url_from_output_path(&output_file, config)?;

        // Remove "index.html" for pretty URLs and wrap in UrlPath
        let permalink = if full_path_url.ends_with("/index.html") {
            UrlPath::from_page(full_path_url.trim_end_matches("index.html"))
        } else {
            UrlPath::from_page(&full_path_url)
        };

        let full_url = format!("{base_url}{permalink}");
        let lastmod = fs::metadata(&source).and_then(|m| m.modified()).ok();

        Ok(Self {
            route: PageRoute {
                source,
                is_index,
                is_404,
                colocated_dir,
                permalink,
                output_file,
                output_dir,
                full_url,
                relative,
            },
            lastmod,
            content_meta: None,
            compiled_html: None,
        })
    }

    /// Set content metadata and check for draft status.
    ///
    /// Returns `Some(self)` if not a draft, `None` if draft.
    #[allow(dead_code)] // Utility method for future use
    pub fn with_content(mut self, content: Option<PageMeta>) -> Option<Self> {
        if content.as_ref().is_some_and(|c| c.draft) {
            return None;
        }
        self.content_meta = content;
        Some(self)
    }

    /// Apply custom permalink from PageMeta if present.
    ///
    /// Updates route.permalink, route.output_file, route.output_dir, and route.full_url.
    /// Call this after setting content_meta.
    pub fn apply_custom_permalink(&mut self, config: &SiteConfig) {
        let custom_permalink = self
            .content_meta
            .as_ref()
            .and_then(|m| m.permalink.as_ref());

        let Some(custom) = custom_permalink else {
            return;
        };

        // Create UrlPath (handles normalization: leading/trailing slashes)
        let permalink = UrlPath::from_page(custom);

        // Update output paths based on new permalink
        let paths = config.paths();
        let output_root = paths.output_dir();
        let base_url = config.site
            .info
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');

        // Build output file path from permalink
        // /custom/path/ → output_root/custom/path/index.html
        let rel_path = permalink
            .as_str()
            .trim_start_matches('/')
            .trim_end_matches('/');
        let output_file = if rel_path.is_empty() {
            output_root.join("index.html")
        } else {
            output_root.join(rel_path).join("index.html")
        };
        let output_dir = output_file.parent().unwrap_or(Path::new("")).to_path_buf();
        let full_url = format!("{base_url}{permalink}");

        // Update route
        self.route.permalink = permalink;
        self.route.output_file = output_file;
        self.route.output_dir = output_dir;
        self.route.full_url = full_url;
    }

    /// Get lastmod as YYYY-MM-DD string for sitemap.
    pub fn lastmod_ymd(&self) -> Option<String> {
        let modified = self.lastmod?;
        let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
        #[allow(clippy::cast_possible_wrap)] // Safe: seconds/86400 fits in i64
        let days = duration.as_secs() as i64 / 86400;
        let (year, month, day) = days_to_ymd(days);
        Some(format!("{year:04}-{month:02}-{day:02}"))
    }
}

// ============================================================================
// Page Collection
// ============================================================================

/// Collection of all compiled pages in the site.
#[derive(Debug, Default)]
pub struct Pages {
    pub items: Vec<CompiledPage>,
}

impl Pages {
    /// Get iterator over pages.
    pub fn iter(&self) -> impl Iterator<Item = &CompiledPage> {
        self.items.iter()
    }

    /// Number of pages.
    #[allow(dead_code)]
    pub const fn len(&self) -> usize {
        self.items.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// Helper to create a PageRoute for testing
    fn test_route(source: &str, output_file: &str, permalink: &str, full_url: &str) -> PageRoute {
        let source = PathBuf::from(source);
        let is_index = source.file_stem().map(|s| s == "index").unwrap_or(false);
        let output_file = PathBuf::from(output_file);
        let output_dir = output_file.parent().unwrap_or(Path::new("")).to_path_buf();
        let relative = source
            .with_extension("")
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        PageRoute {
            source,
            is_index,
            is_404: false,
            colocated_dir: None,
            permalink: UrlPath::from_page(permalink),
            output_file,
            output_dir,
            full_url: full_url.to_string(),
            relative,
        }
    }

    #[test]
    fn test_lastmod_ymd_some() {
        let days_since_epoch = 20254u64;
        let secs = days_since_epoch * 86400;
        let time = UNIX_EPOCH + Duration::from_secs(secs);

        let page = CompiledPage {
            route: test_route(
                "test.typ",
                "public/test/index.html",
                "/test/",
                "https://example.com/test/",
            ),
            lastmod: Some(time),
            content_meta: None,
            compiled_html: None,
        };

        let ymd = page.lastmod_ymd().unwrap();
        assert!(ymd.len() == 10);
        assert!(ymd.starts_with("2025-"));
    }

    #[test]
    fn test_lastmod_ymd_none() {
        let page = CompiledPage {
            route: test_route(
                "test.typ",
                "public/test/index.html",
                "/test/",
                "https://example.com/test/",
            ),
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        };

        assert_eq!(page.lastmod_ymd(), None);
    }

    #[test]
    fn test_page_route_fields() {
        let route = test_route(
            "content/posts/hello.typ",
            "public/posts/hello/index.html",
            "/posts/hello/",
            "https://example.com/posts/hello/",
        );

        assert_eq!(route.source, PathBuf::from("content/posts/hello.typ"));
        assert_eq!(route.source.parent().unwrap(), Path::new("content/posts"));
        assert!(!route.is_index);
        assert_eq!(
            route.output_file,
            PathBuf::from("public/posts/hello/index.html")
        );
        assert_eq!(route.output_dir, PathBuf::from("public/posts/hello"));
        assert_eq!(route.permalink, "/posts/hello/");
        assert_eq!(route.full_url, "https://example.com/posts/hello/");
    }

    #[test]
    fn test_page_route_index() {
        let route = test_route(
            "content/index.typ",
            "public/index.html",
            "/",
            "https://example.com/",
        );

        assert!(route.is_index);
        assert_eq!(route.permalink, "/");
        assert_eq!(route.full_url, "https://example.com/");
    }

    #[test]
    fn test_page_route_with_prefix() {
        let route = test_route(
            "content/posts/hello.typ",
            "public/blog/posts/hello/index.html",
            "/blog/posts/hello/",
            "https://example.com/blog/posts/hello/",
        );

        assert_eq!(
            route.output_file,
            PathBuf::from("public/blog/posts/hello/index.html")
        );
        assert_eq!(route.permalink, "/blog/posts/hello/");
        assert_eq!(route.full_url, "https://example.com/blog/posts/hello/");
    }

    #[test]
    fn test_page_meta_case_mismatch() {
        // Simulate a case where output dir has uppercase (e.g. "Public")
        // but slug config enforces lowercase.
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let posts_dir = content_dir.join("Posts");
        fs::create_dir_all(&posts_dir).unwrap();

        let source = posts_dir.join("Hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("Public");
        config.build.content = content_dir;

        let page = CompiledPage::from_paths(source, &config).unwrap();

        // Output path: "Public" (preserved) + "posts/hello" (slugified) + "index.html"
        assert!(
            page.route
                .output_file
                .ends_with("Public/posts/hello/index.html")
        );

        // URL path: should be derived correctly
        assert_eq!(page.route.permalink, "/posts/hello/");
    }

    #[test]
    fn test_compiled_page_absolute_output_path() {
        // Issue #38: Test that absolute output paths with uppercase preserve casing
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let posts_dir = content_dir.join("Posts");
        fs::create_dir_all(&posts_dir).unwrap();

        let source = posts_dir.join("Hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().to_path_buf();
        config.build.content = content_dir;

        let page = CompiledPage::from_paths(source, &config).unwrap();

        // Output path should preserve absolute path casing
        assert!(page.route.output_file.ends_with("posts/hello/index.html"));
    }

    #[test]
    fn test_compiled_page_is_index() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("index.typ");
        fs::write(&source, "= Home").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;

        let page = CompiledPage::from_paths(source, &config).unwrap();

        assert!(page.route.is_index);
        assert_eq!(page.route.permalink, "/");
    }

    #[test]
    fn test_compiled_page_source_dir() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let posts_dir = content_dir.join("posts");
        fs::create_dir_all(&posts_dir).unwrap();

        let source = posts_dir.join("hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;

        let page = CompiledPage::from_paths(source, &config).unwrap();

        // source.parent() returns the directory containing the source file
        assert!(page.route.source.parent().unwrap().ends_with("posts"));
    }

    #[test]
    fn test_pages_empty() {
        let pages = Pages::default();
        assert_eq!(pages.len(), 0);
        assert_eq!(pages.iter().count(), 0);
    }

    #[test]
    fn test_pages_with_items() {
        let pages = Pages {
            items: vec![
                CompiledPage {
                    route: test_route(
                        "a.typ",
                        "public/a/index.html",
                        "/a/",
                        "https://example.com/a/",
                    ),
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
                CompiledPage {
                    route: test_route(
                        "b.typ",
                        "public/b/index.html",
                        "/b/",
                        "https://example.com/b/",
                    ),
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
            ],
        };

        assert_eq!(pages.len(), 2);
        assert_eq!(pages.iter().count(), 2);
    }

    #[test]
    fn test_pages_iter_urls() {
        let pages = Pages {
            items: vec![
                CompiledPage {
                    route: test_route(
                        "index.typ",
                        "public/index.html",
                        "/",
                        "https://example.com/",
                    ),
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
                CompiledPage {
                    route: test_route(
                        "posts/hello.typ",
                        "public/posts/hello/index.html",
                        "/posts/hello/",
                        "https://example.com/posts/hello/",
                    ),
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
            ],
        };

        let urls: Vec<_> = pages.iter().map(|p| p.route.full_url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/", "https://example.com/posts/hello/"]
        );
    }

    // ========================================================================
    // PageMeta summary deserialization tests
    // ========================================================================

    #[test]
    fn test_content_meta_summary_preserved_as_json() {
        let json = r#"{"title": "Test", "summary": {"func": "text", "text": "A simple summary"}}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Test".to_string()));
        // summary is preserved as raw JSON, not converted to HTML
        assert_eq!(
            meta.summary,
            Some(serde_json::json!({"func": "text", "text": "A simple summary"}))
        );
    }

    #[test]
    fn test_content_meta_summary_sequence_preserved() {
        let json = r#"{
            "title": "Post",
            "summary": {
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "This is a "},
                    {"func": "link", "dest": "https://example.com", "body": {"func": "text", "text": "link"}},
                    {"func": "text", "text": " in summary"}
                ]
            }
        }"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Post".to_string()));
        // summary is preserved as raw JSON for Content reconstruction
        assert!(meta.summary.is_some());
        let summary = meta.summary.unwrap();
        assert_eq!(summary["func"], "sequence");
    }

    #[test]
    fn test_content_meta_summary_none() {
        let json = r#"{"title": "No Summary"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("No Summary".to_string()));
        assert_eq!(meta.summary, None);
    }

    #[test]
    fn test_content_meta_summary_null() {
        let json = r#"{"title": "Null Summary", "summary": null}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Null Summary".to_string()));
        assert_eq!(meta.summary, None);
    }

    #[test]
    fn test_content_meta_summary_string() {
        // Plain string summary is also preserved
        let json = r#"{"summary": "plain text"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.summary, Some(serde_json::json!("plain text")));
    }

    #[test]
    fn test_content_meta_full() {
        let json = r#"{
            "title": "My Blog Post",
            "summary": {"func": "text", "text": "This is the summary"},
            "date": "2025-01-15",
            "update": "2025-01-20",
            "author": "Alice",
            "draft": false
        }"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("My Blog Post".to_string()));
        assert!(meta.summary.is_some());
        assert_eq!(meta.date, Some("2025-01-15".to_string()));
        assert_eq!(meta.update, Some("2025-01-20".to_string()));
        assert_eq!(meta.author, Some("Alice".to_string()));
        assert!(!meta.draft);
    }

    #[test]
    fn test_content_meta_draft_default() {
        let json = r#"{"title": "Draft Test"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert!(!meta.draft); // default is false
    }

    #[test]
    fn test_content_meta_draft_true() {
        let json = r#"{"title": "Draft", "draft": true}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert!(meta.draft);
    }

    #[test]
    fn test_content_meta_tags_null() {
        let json = r#"{"title": "Test", "tags": null}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn test_content_meta_tags_missing() {
        let json = r#"{"title": "Test"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn test_content_meta_tags_array() {
        let json = r#"{"title": "Test", "tags": ["rust", "web"]}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.tags, vec!["rust", "web"]);
    }

    // ========================================================================
    // Custom permalink tests
    // ========================================================================

    #[test]
    fn test_urlpath_normalization() {
        use crate::core::UrlPath;
        assert_eq!(UrlPath::from_page("/custom/path/").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("custom/path").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("/custom/path").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("custom/path/").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("/").as_str(), "/");
        assert_eq!(UrlPath::from_page("  /spaced/  ").as_str(), "/spaced/");
    }

    #[test]
    fn test_content_meta_permalink() {
        let json = r#"{"title": "Test", "permalink": "/archive/2024/hello/"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.permalink, Some("/archive/2024/hello/".to_string()));
    }

    #[test]
    fn test_content_meta_permalink_missing() {
        let json = r#"{"title": "Test"}"#;
        let meta: PageMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.permalink, None);
    }

    #[test]
    fn test_apply_custom_permalink() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;
        config.site.info.url = Some("https://example.com".to_string());

        let mut page = CompiledPage::from_paths(source, &config).unwrap();

        // Default permalink
        assert_eq!(page.route.permalink, "/hello/");

        // Set custom permalink
        page.content_meta = Some(PageMeta {
            permalink: Some("/archive/2024/custom/".to_string()),
            ..Default::default()
        });
        page.apply_custom_permalink(&config);

        // Verify updated values
        assert_eq!(page.route.permalink, "/archive/2024/custom/");
        assert!(
            page.route
                .output_file
                .ends_with("public/archive/2024/custom/index.html")
        );
        assert!(
            page.route
                .output_dir
                .ends_with("public/archive/2024/custom")
        );
        assert_eq!(
            page.route.full_url,
            "https://example.com/archive/2024/custom/"
        );
    }

    #[test]
    fn test_apply_custom_permalink_normalizes() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;

        let mut page = CompiledPage::from_paths(source, &config).unwrap();

        // Set permalink without leading/trailing slashes
        page.content_meta = Some(PageMeta {
            permalink: Some("custom-slug".to_string()),
            ..Default::default()
        });
        page.apply_custom_permalink(&config);

        // Should be normalized
        assert_eq!(page.route.permalink, "/custom-slug/");
    }

    #[test]
    fn test_apply_custom_permalink_none_no_change() {
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("hello.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;

        let mut page = CompiledPage::from_paths(source, &config).unwrap();
        let original_permalink = page.route.permalink.clone();

        // No custom permalink
        page.content_meta = Some(PageMeta::default());
        page.apply_custom_permalink(&config);

        // Should remain unchanged
        assert_eq!(page.route.permalink, original_permalink);
    }

    #[test]
    fn test_days_to_ymd_epoch() {
        // Unix epoch: 1970-01-01
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_known_date() {
        // 2025-06-15 is day 20254 since epoch
        let (y, m, d) = days_to_ymd(20254);
        assert_eq!((y, m, d), (2025, 6, 15));
    }

    #[test]
    fn test_days_to_ymd_leap_year() {
        // 2024-02-29 is day 19782 since epoch (leap year)
        let (y, m, d) = days_to_ymd(19782);
        assert_eq!((y, m, d), (2024, 2, 29));
    }
}
