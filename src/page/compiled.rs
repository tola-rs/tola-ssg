//! Compiled page and page collection types.

#![allow(dead_code)]

use std::{fs, path::Path, time::SystemTime};

use anyhow::{Result, anyhow};

use crate::asset::url_from_output_path;
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::utils::path::slug::slugify_path;

use super::{PageMeta, PageRoute};

/// Convert days since Unix epoch to (year, month, day)
///
/// Uses a simplified leap year calculation that's accurate for dates
/// from 1970 to ~2100
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

/// Primary metadata structure for a compiled content page
///
/// Contains all path and URL information needed by build, rss and sitemap
/// This is the **single source of truth** for page paths and content metadata
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
/// Note: Previously named `PageMeta`. Renamed to avoid confusion with `page::PageMeta`
#[derive(Debug, Clone)]
pub struct CompiledPage {
    /// Route information (source -> output mapping)
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
    /// Use `apply_meta` to set the content metadata later.
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

        // Check if this is an index file
        let is_index = source.file_stem().map(|s| s == "index").unwrap_or(false);

        // Strip content dir and extension
        let rel_path = source
            .strip_prefix(&content_dir)
            .map_err(|_| anyhow!("File is not in content directory: {}", source.display()))?;

        // Check if this is the 404 page (compare source path with config.site.not_found)
        // not_found is relative to site root, so we compare with source relative to root
        let is_404 = config
            .site
            .not_found
            .as_ref()
            .is_some_and(|nf| config.root_relative(&source) == *nf);

        // Use file_stem to handle any supported extension (.typ, .md, etc.)
        // For index files (xxx/index.typ), use parent directory as relative path
        let relative = if is_index {
            rel_path
                .parent()
                .and_then(|p| p.to_str())
                .map(|s| s.to_owned())
                .unwrap_or_default()
        } else {
            rel_path
                .with_extension("")
                .to_str()
                .ok_or_else(|| anyhow!("Invalid path encoding"))?
                .to_owned()
        };

        let is_root_index = relative.is_empty() || relative == "index";

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

        // Compute URL path from the final HTML path to ensure consistency
        let full_path_url = url_from_output_path(&output_file, config)?;

        // Remove "index.html" for pretty URLs and wrap in UrlPath
        let permalink = if full_path_url.ends_with("/index.html") {
            UrlPath::from_page(full_path_url.trim_end_matches("index.html"))
        } else {
            UrlPath::from_page(&full_path_url)
        };

        let full_url = permalink.canonical_url(config.site.info.url.as_deref());
        let lastmod = fs::metadata(&source).and_then(|m| m.modified()).ok();

        Ok(Self {
            route: PageRoute {
                source,
                is_index,
                is_404,
                permalink,
                output_file,
                output_dir,
                full_url,
            },
            lastmod,
            content_meta: None,
            compiled_html: None,
        })
    }

    /// Create `CompiledPage` from source and apply metadata-driven route updates.
    pub fn from_paths_with_meta(
        source: impl AsRef<Path>,
        config: &SiteConfig,
        meta: Option<PageMeta>,
    ) -> Result<Self> {
        let mut page = Self::from_paths(source, config)?;
        page.apply_meta(meta, config);
        Ok(page)
    }

    /// Apply metadata and update permalink/output route if needed.
    pub fn apply_meta(&mut self, meta: Option<PageMeta>, config: &SiteConfig) {
        self.content_meta = meta;
        self.apply_custom_permalink(config);
    }

    /// Apply custom permalink from PageMeta if present.
    ///
    /// Updates route.permalink, route.output_file, route.output_dir, and route.full_url.
    /// Call this after setting content_meta.
    fn apply_custom_permalink(&mut self, config: &SiteConfig) {
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

        // Build output file path from permalink.
        let output_file = permalink.output_html_path(&output_root);
        let output_dir = output_file.parent().unwrap_or(Path::new("")).to_path_buf();
        let full_url = permalink.canonical_url(config.site.info.url.as_deref());

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

/// Collection of all compiled pages in the site
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    /// Helper to create a PageRoute for testing
    fn test_route(source: &str, output_file: &str, permalink: &str, full_url: &str) -> PageRoute {
        let source = PathBuf::from(source);
        let is_index = source.file_stem().map(|s| s == "index").unwrap_or(false);
        let output_file = PathBuf::from(output_file);
        let output_dir = output_file.parent().unwrap_or(Path::new("")).to_path_buf();

        PageRoute {
            source,
            is_index,
            is_404: false,
            permalink: UrlPath::from_page(permalink),
            output_file,
            output_dir,
            full_url: full_url.to_string(),
        }
    }

    fn temp_source_page(source_rel: &str, body: &str) -> (TempDir, PathBuf, SiteConfig) {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let source = content_dir.join(source_rel);
        if let Some(parent) = source.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&source, body).unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.content = content_dir;

        (dir, source, config)
    }

    fn parse_meta(json: &str) -> PageMeta {
        serde_json::from_str(json).unwrap()
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
    fn test_page_route_cases() {
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

        let index = test_route(
            "content/index.typ",
            "public/index.html",
            "/",
            "https://example.com/",
        );
        assert!(index.is_index);
        assert_eq!(index.permalink, "/");
        assert_eq!(index.full_url, "https://example.com/");

        let prefixed = test_route(
            "content/posts/hello.typ",
            "public/blog/posts/hello/index.html",
            "/blog/posts/hello/",
            "https://example.com/blog/posts/hello/",
        );
        assert_eq!(
            prefixed.output_file,
            PathBuf::from("public/blog/posts/hello/index.html")
        );
        assert_eq!(prefixed.permalink, "/blog/posts/hello/");
        assert_eq!(prefixed.full_url, "https://example.com/blog/posts/hello/");
    }

    #[test]
    fn test_page_meta_case_mismatch() {
        // Simulate a case where output dir has uppercase (e.g. "Public")
        // but slug config enforces lowercase.
        let (dir, source, mut config) = temp_source_page("Posts/Hello.typ", "= Hello");
        config.build.output = dir.path().join("Public");

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
        let (dir, source, mut config) = temp_source_page("Posts/Hello.typ", "= Hello");
        config.build.output = dir.path().to_path_buf();

        let page = CompiledPage::from_paths(source, &config).unwrap();

        // Output path should preserve absolute path casing
        assert!(page.route.output_file.ends_with("posts/hello/index.html"));
    }

    #[test]
    fn test_compiled_page_index_route() {
        let (_dir, source, config) = temp_source_page("index.typ", "= Home");
        let page = CompiledPage::from_paths(source, &config).unwrap();

        assert!(page.route.is_index);
        assert_eq!(page.route.permalink, "/");
        assert!(page.route.output_file.ends_with("public/index.html"));
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
        let meta = parse_meta(
            r#"{"title": "Test", "summary": {"func": "text", "text": "A simple summary"}}"#,
        );
        assert_eq!(meta.title, Some("Test".to_string()));
        // summary is preserved as raw JSON, not converted to HTML
        assert_eq!(
            meta.summary,
            Some(serde_json::json!({"func": "text", "text": "A simple summary"}))
        );
    }

    #[test]
    fn test_content_meta_summary_sequence_preserved() {
        let meta = parse_meta(
            r#"{
            "title": "Post",
            "summary": {
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "This is a "},
                    {"func": "link", "dest": "https://example.com", "body": {"func": "text", "text": "link"}},
                    {"func": "text", "text": " in summary"}
                ]
            }
        }"#,
        );
        assert_eq!(meta.title, Some("Post".to_string()));
        // summary is preserved as raw JSON for Content reconstruction
        assert!(meta.summary.is_some());
        let summary = meta.summary.unwrap();
        assert_eq!(summary["func"], "sequence");
    }

    #[test]
    fn test_content_meta_missing_or_null_summary_yields_none() {
        for json in [
            r#"{"title": "No Summary"}"#,
            r#"{"title": "Null Summary", "summary": null}"#,
        ] {
            let meta = parse_meta(json);
            assert_eq!(meta.summary, None);
        }
    }

    #[test]
    fn test_content_meta_summary_string() {
        // Plain string summary is also preserved
        let meta = parse_meta(r#"{"summary": "plain text"}"#);
        assert_eq!(meta.summary, Some(serde_json::json!("plain text")));
    }

    #[test]
    fn test_content_meta_full() {
        let meta = parse_meta(
            r#"{
            "title": "My Blog Post",
            "summary": {"func": "text", "text": "This is the summary"},
            "date": "datetime(year: 2025, month: 1, day: 15)",
            "update": "datetime(year: 2025, month: 1, day: 20)",
            "author": "Alice",
            "draft": false
        }"#,
        );
        assert_eq!(meta.title, Some("My Blog Post".to_string()));
        assert!(meta.summary.is_some());
        assert_eq!(meta.date, Some("2025-01-15".to_string()));
        assert_eq!(meta.update, Some("2025-01-20".to_string()));
        assert_eq!(meta.author, Some("Alice".to_string()));
        assert!(!meta.draft);
    }

    #[test]
    fn test_content_meta_draft_cases() {
        let default_meta = parse_meta(r#"{"title": "Draft Test"}"#);
        assert!(!default_meta.draft);

        let draft_meta = parse_meta(r#"{"title": "Draft", "draft": true}"#);
        assert!(draft_meta.draft);
    }

    #[test]
    fn test_content_meta_tags_default_to_empty() {
        for json in [r#"{"title": "Test", "tags": null}"#, r#"{"title": "Test"}"#] {
            let meta = parse_meta(json);
            assert!(meta.tags.is_empty());
        }
    }

    #[test]
    fn test_content_meta_tags_array() {
        let meta = parse_meta(r#"{"title": "Test", "tags": ["rust", "web"]}"#);
        assert_eq!(meta.tags, vec!["rust", "web"]);
    }

    // ========================================================================
    // Custom permalink tests
    // ========================================================================

    #[test]
    fn test_urlpath_normalization() {
        use crate::core::UrlPath;
        assert_eq!(
            UrlPath::from_page("/custom/path/").as_str(),
            "/custom/path/"
        );
        assert_eq!(UrlPath::from_page("custom/path").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("/custom/path").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("custom/path/").as_str(), "/custom/path/");
        assert_eq!(UrlPath::from_page("/").as_str(), "/");
        assert_eq!(UrlPath::from_page("  /spaced/  ").as_str(), "/spaced/");
    }

    #[test]
    fn test_apply_meta_updates_route_from_custom_permalink() {
        let (_dir, source, mut config) = temp_source_page("hello.typ", "= Hello");
        config.site.info.url = Some("https://example.com".to_string());

        let mut page = CompiledPage::from_paths(source, &config).unwrap();

        // Default permalink
        assert_eq!(page.route.permalink, "/hello/");

        // Set custom permalink via unified metadata application path.
        page.apply_meta(
            Some(PageMeta {
                permalink: Some("/archive/2024/custom/".to_string()),
                ..Default::default()
            }),
            &config,
        );

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
    fn test_apply_meta_normalizes_permalink() {
        let (_dir, source, config) = temp_source_page("hello.typ", "= Hello");
        let mut page = CompiledPage::from_paths(source, &config).unwrap();

        // Set permalink without leading/trailing slashes.
        page.apply_meta(
            Some(PageMeta {
                permalink: Some("custom-slug".to_string()),
                ..Default::default()
            }),
            &config,
        );

        // Should be normalized
        assert_eq!(page.route.permalink, "/custom-slug/");
    }

    #[test]
    fn test_apply_meta_without_permalink_keeps_route() {
        let (_dir, source, config) = temp_source_page("hello.typ", "= Hello");
        let mut page = CompiledPage::from_paths(source, &config).unwrap();
        let original_permalink = page.route.permalink.clone();

        // No custom permalink.
        page.apply_meta(Some(PageMeta::default()), &config);

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
