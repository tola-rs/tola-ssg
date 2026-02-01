//! Page metadata collection and caching.
//!
//! `PageMeta` is the **primary metadata structure** for content pages,
//! containing all path and URL information needed across the build pipeline.
//!
//! # Constants
//!
//! - [`TOLA_META_LABEL`]: The typst label name for metadata queries (`"tola-meta"`)
//!
//! # Architecture
//!
//! ```text
//! build_site()
//!     │
//!     └── process_content() ──► compile_typst_with_meta()
//!                                       │
//!                                       ├── HTML output (written to disk)
//!                                       └── ContentMeta (optional, from <tola-meta>)
//!                                               │
//!                                               ▼
//!                                         PageMeta::with_content()
//!                                               │
//!                                               ▼
//!                                    Pages { items: Vec<PageMeta> }
//!                                               │
//!                          ┌────────────────────┴────────────────────┐
//!                          ▼                                         ▼
//!                    build_rss()                              build_sitemap()
//!                    (uses page.content_meta)                 (uses page.paths)
//! ```
//!
//! # Key Optimization
//!
//! Each `.typ` file is compiled **exactly once**. During compilation:
//! - HTML is generated and written to disk
//! - Metadata (from `<tola-meta>` label) is extracted from the same Document
//!
//! This avoids the previous architecture where files were compiled twice
//! (once for HTML, once for metadata query).
//!
//! # Usage
//!
//! ```ignore
//! // In build_site, pages are collected during compilation:
//! let pages = content_files
//!     .par_iter()
//!     .filter_map(|path| process_content(path, config, ...)?)
//!     .collect();
//!
//! for page in pages.iter() {
//!     // Path info:
//!     // - page.paths.html: output HTML path
//!     // - page.paths.relative: for logging
//!     // - page.paths.full_url: complete URL
//!     //
//!     // Content metadata (from <tola-meta>):
//!     // - page.content_meta.title
//!     // - page.content_meta.summary
//!     // - page.content_meta.date
//! }
//! ```

use crate::{config::SiteConfig, utils::slug::slugify_path};
use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

// ============================================================================
// Constants
// ============================================================================

/// Label name for typst metadata queries.
///
/// Used to extract page metadata from `#metadata(...) <tola-meta>` in typst files.
pub const TOLA_META_LABEL: &str = "tola-meta";

// ============================================================================
// Asset Metadata
// ============================================================================

/// Metadata for a static asset file.
///
/// Handles path resolution for assets, ensuring consistent URL generation
/// and output path calculation.
#[derive(Debug, Clone)]
pub struct AssetMeta {
    /// Path information
    pub paths: AssetPaths,
}

/// Path information for an asset.
#[derive(Debug, Clone)]
pub struct AssetPaths {
    /// Source file path
    pub source: PathBuf,
    /// Output file path (in public directory)
    pub dest: PathBuf,
    /// Relative path from assets root (for logging)
    pub relative: String,
    /// URL path (for linking)
    pub url: String,
}

impl AssetMeta {
    /// Create `AssetMeta` from a source path.
    pub fn from_source(source: PathBuf, config: &SiteConfig) -> Result<Self> {
        let assets_dir = &config.build.assets;
        let output_dir = config.paths().output_dir();

        let relative = source
            .strip_prefix(assets_dir)
            .map_err(|_| anyhow!("File is not in assets directory: {}", source.display()))?
            .to_str()
            .ok_or_else(|| anyhow!("Invalid path encoding"))?
            .to_owned();

        let dest = output_dir.join(&relative);
        let url = url_from_output_path(&dest, config)?;

        Ok(Self {
            paths: AssetPaths {
                source,
                dest,
                relative,
                url,
            },
        })
    }
}

/// Generate a URL path from an output file path.
///
/// Handles path prefix stripping and cross-platform separators.
pub fn url_from_output_path(path: &Path, config: &SiteConfig) -> Result<String> {
    let output_root = &config.build.output;

    // Strip output root
    let rel_to_output = path
        .strip_prefix(output_root)
        .map_err(|_| anyhow!("Path is not in output directory: {}", path.display()))?;

    // Convert to string and ensure forward slashes
    let path_str = rel_to_output.to_string_lossy().replace('\\', "/");

    // Ensure it starts with /
    let url = if path_str.starts_with('/') {
        path_str
    } else {
        format!("/{path_str}")
    };

    Ok(url)
}

// ============================================================================
// Page Metadata
// ============================================================================

/// Content metadata from `#metadata(...) <tola-meta>` in typst files.
///
/// Deserialized directly from typst `Value` via serde.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContentMeta {
    pub title: Option<String>,
    /// Custom output URL
    #[serde(default)]
    pub url: Option<PathBuf>,
    /// Summary content (converted to HTML from Typst elements)
    #[serde(default, deserialize_with = "deserialize_summary")]
    pub summary: Option<String>,
    pub date: Option<String>,
    #[allow(dead_code)] // Reserved for future use
    pub update: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub draft: bool,
    /// Tags for categorizing the page.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Primary metadata structure for a content page.
///
/// Contains all path and URL information needed by build, rss and sitemap.
/// This is the **single source of truth** for page paths and content metadata.
///
/// # Fields
///
/// | Field | Example | Used By |
/// |-------|---------|---------|
/// | `paths.source` | `content/posts/hello.typ` | build, rss query |
/// | `paths.html` | `public/posts/hello/index.html` | build output |
/// | `paths.relative` | `posts/hello` | logging |
/// | `paths.url_path` | `/posts/hello/` | URL construction |
/// | `paths.full_url` | `https://example.com/posts/hello/` | rss, sitemap |
/// | `lastmod` | `SystemTime` | sitemap |
/// | `content_meta` | `ContentMeta` | rss (title/summary/date) |
/// | `compiled_html` | `Vec<u8>` | Lib mode pre-compiled HTML |
#[derive(Debug, Clone)]
pub struct PageMeta {
    /// Path information
    pub paths: PagePaths,
    /// Last modification time of the HTML file
    pub lastmod: Option<SystemTime>,
    /// Content metadata from `<tola-meta>` (None if not present)
    pub content_meta: Option<ContentMeta>,
    /// Pre-compiled HTML content (Lib mode only, None for CLI mode)
    pub compiled_html: Option<Vec<u8>>,
}

/// Path information for a page.
#[derive(Debug, Clone)]
pub struct PagePaths {
    /// Source .typ file path
    pub source: PathBuf,
    /// Generated HTML file path (includes `path_prefix`)
    pub html: PathBuf,
    /// Relative path without extension (for logging)
    pub relative: String,
    /// URL path component (includes `path_prefix`, e.g., `/prefix/posts/hello/`)
    #[allow(dead_code)] // Reserved for future use
    pub url_path: String,
    /// Full URL including base (e.g., `https://example.com/posts/hello/`)
    pub full_url: String,
}

impl PageMeta {
    /// Create `PageMeta` from a source .typ file path without querying metadata.
    ///
    /// This is the lightweight version that only computes paths.
    /// Use `with_content` to set the content metadata later.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - File is not in content directory
    /// - File is not a .typ file
    pub fn from_paths(
        source: PathBuf,
        config: &SiteConfig,
        content_meta: Option<ContentMeta>,
    ) -> Result<Self> {
        let content_dir = &config.build.content;
        let paths = config.paths();
        let output_dir = paths.output_dir();
        let base_url = config
            .base
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');

        // Strip content dir and .typ extension
        let relative = source
            .strip_prefix(content_dir)
            .map_err(|_| anyhow!("File is not in content directory: {}", source.display()))?
            .to_str()
            .ok_or_else(|| anyhow!("Invalid path encoding"))?
            .strip_suffix(".typ")
            .ok_or_else(|| anyhow!("Not a .typ file: {}", source.display()))?
            .to_owned();

        let is_root_index = relative == "index";

        // Compute HTML output path
        // Only slugify the relative path part to preserve output dir and index.html
        let html = if let Some(url) = content_meta.as_ref().and_then(|meta| meta.url.clone()) {
            let slugified_relative = slugify_path(Path::new(&url), config);
            output_dir.join(slugified_relative)
        } else if is_root_index {
            output_dir
        } else {
            let slugified_relative = slugify_path(Path::new(&relative), config);
            output_dir.join(slugified_relative)
        };
        let html = html.join("index.html");

        // Compute URL path from the final HTML path to ensure consistency
        let full_path_url = url_from_output_path(&html, config)?;

        // Remove "index.html" for pretty URLs
        let url_path = if full_path_url.ends_with("/index.html") {
            full_path_url.trim_end_matches("index.html").to_string()
        } else {
            full_path_url
        };

        let full_url = format!("{base_url}{url_path}");
        let lastmod = fs::metadata(&source).and_then(|m| m.modified()).ok();

        Ok(Self {
            paths: PagePaths {
                source,
                html,
                relative,
                url_path,
                full_url,
            },
            lastmod,
            content_meta,
            compiled_html: None,
        })
    }

    /// Set content metadata and check for draft status.
    ///
    /// Returns `Some(self)` if not a draft, `None` if draft.
    #[allow(dead_code)] // Utility method for future use
    pub fn with_content(mut self, content: Option<ContentMeta>) -> Option<Self> {
        if content.as_ref().is_some_and(|c| c.draft) {
            return None;
        }
        self.content_meta = content;
        Some(self)
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

/// Collection of all pages in the site.
#[derive(Debug, Default)]
pub struct Pages {
    pub items: Vec<PageMeta>,
}

impl Pages {
    /// Get iterator over pages.
    pub fn iter(&self) -> impl Iterator<Item = &PageMeta> {
        self.items.iter()
    }

    /// Number of pages.
    #[allow(dead_code)]
    pub const fn len(&self) -> usize {
        self.items.len()
    }
}

// ============================================================================
// Typst Element Parsing (for summary field)
// ============================================================================

/// Deserialize summary field: parse Typst elements and convert to HTML.
fn deserialize_summary<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        Some(v) => {
            // Handle simple strings directly: e.g.: `summary: "hello world"`
            if let Some(s) = v.as_str() {
                return Ok(Some(html_escape(s).into_owned()));
            }

            // Handle Typst content elements: e.g.: `summary: [hello world, _italic_, $x + y$]`
            let elem: TypstElement = serde_json::from_value(v)
                .map_err(|e| D::Error::custom(format!("Invalid summary format: {e}")))?;
            Ok(Some(elem.to_html()))
        }
        None => Ok(None),
    }
}

/// Typst content element for summary field deserialization.
///
/// Parses JSON-serialized Typst content and converts to HTML.
///
/// # Supported Elements
///
/// | Element    | HTML Output               |
/// |------------|---------------------------|
/// | Space      | ` ` (space)               |
/// | Linebreak  | `<br/>`                   |
/// | Text       | Escaped text              |
/// | Strike     | `<s>text</s>`             |
/// | Link       | `<a href="...">text</a>`  |
/// | Sequence   | Concatenated children     |
/// | Unknown    | Empty string (ignored)    |
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "func", rename_all = "lowercase")]
enum TypstElement {
    Space,
    Linebreak,
    Text {
        text: String,
    },
    Strike {
        text: String,
    },
    Link {
        dest: String,
        body: Box<Self>,
    },
    Sequence {
        children: Vec<Self>,
    },
    #[serde(other)]
    Unknown,
}

impl TypstElement {
    /// Convert Typst element to HTML string.
    fn to_html(&self) -> String {
        match self {
            Self::Space => " ".into(),
            Self::Linebreak => "<br/>".into(),
            Self::Text { text } => html_escape(text).into_owned(),
            Self::Strike { text } => format!("<s>{}</s>", html_escape(text)),
            Self::Link { dest, body } => {
                format!("<a href=\"{dest}\">{}</a>", body.to_html())
            }
            Self::Sequence { children } => children.iter().map(Self::to_html).collect(),
            Self::Unknown => String::new(),
        }
    }
}

/// Escape HTML special characters.
///
/// Uses `Cow` to avoid allocation when no escaping is needed.
#[inline]
fn html_escape(s: &str) -> Cow<'_, str> {
    // Fast path: check if escaping is needed
    if !s.contains(['<', '>', '&', '"']) {
        return Cow::Borrowed(s);
    }

    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '&' => result.push_str("&amp;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert days since UNIX epoch (1970-01-01) to (year, month, day).
///
/// Uses Howard Hinnant's date algorithms for efficient calendar calculations.
/// See: <http://howardhinnant.github.io/date_algorithms.html>
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Shift epoch from 1970-01-01 to 0000-03-01
    let z = days + 719_468;

    // Calculate era (400-year period)
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;

    // Day of era [0, 146096]
    let doe = (z - era * 146_097) as u32;

    // Year of era [0, 399]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;

    // Year
    let y = yoe as i64 + era * 400;

    // Day of year [0, 365]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);

    // Month [0, 11] -> [3, 14]
    let mp = (5 * doy + 2) / 153;

    // Day [1, 31]
    let d = doy - (153 * mp + 2) / 5 + 1;

    // Month [1, 12]
    let m = if mp < 10 { mp + 3 } else { mp - 9 };

    // Adjust year for Jan/Feb
    let y = if m <= 2 { y + 1 } else { y };

    (y as i32, m, d)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn test_days_to_ymd_unix_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_one_year() {
        assert_eq!(days_to_ymd(365), (1971, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_leap_year() {
        assert_eq!(days_to_ymd(730), (1972, 1, 1));
        assert_eq!(days_to_ymd(730 + 31 + 28), (1972, 2, 29));
    }

    #[test]
    fn test_days_to_ymd_2025() {
        assert_eq!(days_to_ymd(20089), (2025, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_negative() {
        assert_eq!(days_to_ymd(-1), (1969, 12, 31));
    }

    #[test]
    fn test_days_to_ymd_century_boundary() {
        assert_eq!(days_to_ymd(10957), (2000, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_end_of_year() {
        assert_eq!(days_to_ymd(364), (1970, 12, 31));
    }

    #[test]
    fn test_lastmod_ymd_some() {
        let days_since_epoch = 20254u64;
        let secs = days_since_epoch * 86400;
        let time = UNIX_EPOCH + Duration::from_secs(secs);

        let page = PageMeta {
            paths: PagePaths {
                source: PathBuf::from("test.typ"),
                html: PathBuf::from("public/test/index.html"),
                relative: "test".to_string(),
                url_path: "/test/".to_string(),
                full_url: "https://example.com/test/".to_string(),
            },
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
        let page = PageMeta {
            paths: PagePaths {
                source: PathBuf::from("test.typ"),
                html: PathBuf::from("public/test/index.html"),
                relative: "test".to_string(),
                url_path: "/test/".to_string(),
                full_url: "https://example.com/test/".to_string(),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        };

        assert_eq!(page.lastmod_ymd(), None);
    }

    #[test]
    fn test_page_meta_fields() {
        let page = PageMeta {
            paths: PagePaths {
                source: PathBuf::from("content/posts/hello.typ"),
                html: PathBuf::from("public/posts/hello/index.html"),
                relative: "posts/hello".to_string(),
                url_path: "/posts/hello/".to_string(),
                full_url: "https://example.com/posts/hello/".to_string(),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        };

        assert_eq!(page.paths.source, PathBuf::from("content/posts/hello.typ"));
        assert_eq!(
            page.paths.html,
            PathBuf::from("public/posts/hello/index.html")
        );
        assert_eq!(page.paths.relative, "posts/hello");
        assert_eq!(page.paths.url_path, "/posts/hello/");
        assert_eq!(page.paths.full_url, "https://example.com/posts/hello/");
    }

    #[test]
    fn test_page_meta_root_index() {
        let page = PageMeta {
            paths: PagePaths {
                source: PathBuf::from("content/index.typ"),
                html: PathBuf::from("public/index.html"),
                relative: "index".to_string(),
                url_path: "/".to_string(),
                full_url: "https://example.com/".to_string(),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        };

        assert_eq!(page.paths.relative, "index");
        assert_eq!(page.paths.url_path, "/");
        assert_eq!(page.paths.full_url, "https://example.com/");
    }

    #[test]
    fn test_page_meta_with_path_prefix() {
        let page = PageMeta {
            paths: PagePaths {
                source: PathBuf::from("content/posts/hello.typ"),
                html: PathBuf::from("public/blog/posts/hello/index.html"),
                relative: "posts/hello".to_string(),
                url_path: "/blog/posts/hello/".to_string(),
                full_url: "https://example.com/blog/posts/hello/".to_string(),
            },
            lastmod: None,
            content_meta: None,
            compiled_html: None,
        };

        assert_eq!(
            page.paths.html,
            PathBuf::from("public/blog/posts/hello/index.html")
        );
        assert_eq!(page.paths.url_path, "/blog/posts/hello/");
        assert_eq!(page.paths.full_url, "https://example.com/blog/posts/hello/");
    }

    #[test]
    fn test_page_meta_case_mismatch() {
        // Simulate a case where output dir has uppercase (e.g. "Public")
        // but slug config enforces lowercase.
        // The logic should preserve the output dir casing while slugifying the content path.
        let mut config = SiteConfig::default();
        config.build.output = PathBuf::from("Public");
        config.build.content = PathBuf::from("content");

        // Leak config to get 'static lifetime required by from_paths
        let config: &SiteConfig = Box::leak(Box::new(config));

        let source = PathBuf::from("content/Posts/Hello.typ");
        let page = PageMeta::from_paths(source, config, None).unwrap();

        // Output path: "Public" (preserved) + "posts/hello" (slugified) + "index.html"
        assert_eq!(
            page.paths.html,
            PathBuf::from("Public/posts/hello/index.html")
        );

        // URL path: should be derived correctly despite "Public" vs "public" mismatch if we were slugifying everything
        assert_eq!(page.paths.url_path, "/posts/hello/");
    }

    #[test]
    fn test_page_meta_absolute_output_path() {
        // Issue #38: Test that absolute output paths with uppercase preserve casing
        let mut config = SiteConfig::default();
        config.build.output = PathBuf::from("/tmp/Personal/25FW/website");
        config.build.content = PathBuf::from("/tmp/Personal/25FW/website/content");

        let config: &SiteConfig = Box::leak(Box::new(config));

        let source = PathBuf::from("/tmp/Personal/25FW/website/content/Posts/Hello.typ");
        let page = PageMeta::from_paths(source, config, None).unwrap();

        // Output path should preserve absolute path casing
        assert_eq!(
            page.paths.html,
            PathBuf::from("/tmp/Personal/25FW/website/posts/hello/index.html")
        );
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
                PageMeta {
                    paths: PagePaths {
                        source: PathBuf::from("a.typ"),
                        html: PathBuf::from("public/a/index.html"),
                        relative: "a".to_string(),
                        url_path: "/a/".to_string(),
                        full_url: "https://example.com/a/".to_string(),
                    },
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
                PageMeta {
                    paths: PagePaths {
                        source: PathBuf::from("b.typ"),
                        html: PathBuf::from("public/b/index.html"),
                        relative: "b".to_string(),
                        url_path: "/b/".to_string(),
                        full_url: "https://example.com/b/".to_string(),
                    },
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
                PageMeta {
                    paths: PagePaths {
                        source: PathBuf::from("index.typ"),
                        html: PathBuf::from("public/index.html"),
                        relative: "index".to_string(),
                        url_path: "/".to_string(),
                        full_url: "https://example.com/".to_string(),
                    },
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
                PageMeta {
                    paths: PagePaths {
                        source: PathBuf::from("posts/hello.typ"),
                        html: PathBuf::from("public/posts/hello/index.html"),
                        relative: "posts/hello".to_string(),
                        url_path: "/posts/hello/".to_string(),
                        full_url: "https://example.com/posts/hello/".to_string(),
                    },
                    lastmod: None,
                    content_meta: None,
                    compiled_html: None,
                },
            ],
        };

        let urls: Vec<_> = pages.iter().map(|p| p.paths.full_url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/", "https://example.com/posts/hello/"]
        );
    }

    // ========================================================================
    // TypstElement tests
    // ========================================================================

    #[test]
    fn test_typst_element_text() {
        let json = r#"{"func": "text", "text": "Hello World"}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert_eq!(elem.to_html(), "Hello World");
    }

    #[test]
    fn test_typst_element_space() {
        let json = r#"{"func": "space"}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Space));
        assert_eq!(elem.to_html(), " ");
    }

    #[test]
    fn test_typst_element_linebreak() {
        let json = r#"{"func": "linebreak"}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Linebreak));
        assert_eq!(elem.to_html(), "<br/>");
    }

    #[test]
    fn test_typst_element_strike() {
        let json = r#"{"func": "strike", "text": "deleted"}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert_eq!(elem.to_html(), "<s>deleted</s>");
    }

    #[test]
    fn test_typst_element_link() {
        let json = r#"{"func": "link", "dest": "https://example.com", "body": {"func": "text", "text": "click here"}}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        if let TypstElement::Link { dest, body } = &elem {
            assert_eq!(dest, "https://example.com");
            assert!(matches!(body.as_ref(), TypstElement::Text { text } if text == "click here"));
        } else {
            panic!("Expected Link element");
        }
        assert_eq!(
            elem.to_html(),
            r#"<a href="https://example.com">click here</a>"#
        );
    }

    #[test]
    fn test_typst_element_sequence() {
        let json = r#"{"func": "sequence", "children": [{"func": "text", "text": "Hello"}, {"func": "space"}, {"func": "text", "text": "World"}]}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        if let TypstElement::Sequence { children } = &elem {
            assert_eq!(children.len(), 3);
            assert!(matches!(&children[0], TypstElement::Text { text } if text == "Hello"));
            assert!(matches!(&children[1], TypstElement::Space));
            assert!(matches!(&children[2], TypstElement::Text { text } if text == "World"));
        } else {
            panic!("Expected Sequence element");
        }
        assert_eq!(elem.to_html(), "Hello World");
    }

    #[test]
    fn test_typst_element_unknown() {
        let json = r#"{"func": "some_unknown_func"}"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Unknown));
        assert_eq!(elem.to_html(), "");
    }

    #[test]
    fn test_typst_element_nested_sequence() {
        let json = r#"{
            "func": "sequence",
            "children": [
                {"func": "text", "text": "Start "},
                {"func": "link", "dest": "https://rust-lang.org", "body": {"func": "text", "text": "Rust"}},
                {"func": "text", "text": " is great"}
            ]
        }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert_eq!(
            elem.to_html(),
            r#"Start <a href="https://rust-lang.org">Rust</a> is great"#
        );
    }

    // ========================================================================
    // html_escape tests
    // ========================================================================

    #[test]
    fn test_html_escape_plain() {
        assert_eq!(html_escape("hello world"), "hello world");
    }

    #[test]
    fn test_html_escape_special_chars() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
        assert_eq!(html_escape("say \"hi\""), "say &quot;hi&quot;");
    }

    #[test]
    fn test_html_escape_mixed() {
        assert_eq!(
            html_escape("<a href=\"#\">link & text</a>"),
            "&lt;a href=&quot;#&quot;&gt;link &amp; text&lt;/a&gt;"
        );
    }

    #[test]
    fn test_html_escape_empty() {
        assert_eq!(html_escape(""), "");
    }

    // ========================================================================
    // ContentMeta summary deserialization tests
    // ========================================================================

    #[test]
    fn test_content_meta_summary_text() {
        let json = r#"{"title": "Test", "summary": {"func": "text", "text": "A simple summary"}}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Test".to_string()));
        assert_eq!(meta.summary, Some("A simple summary".to_string()));
    }

    #[test]
    fn test_content_meta_summary_sequence() {
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
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Post".to_string()));
        assert_eq!(
            meta.summary,
            Some(r#"This is a <a href="https://example.com">link</a> in summary"#.to_string())
        );
    }

    #[test]
    fn test_content_meta_summary_none() {
        let json = r#"{"title": "No Summary"}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("No Summary".to_string()));
        assert_eq!(meta.summary, None);
    }

    #[test]
    fn test_content_meta_summary_null() {
        let json = r#"{"title": "Null Summary", "summary": null}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("Null Summary".to_string()));
        assert_eq!(meta.summary, None);
    }

    #[test]
    fn test_content_meta_summary_with_html_escape() {
        let json = r#"{"summary": {"func": "text", "text": "Use <code> & \"quotes\""}}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(
            meta.summary,
            Some("Use &lt;code&gt; &amp; &quot;quotes&quot;".to_string())
        );
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
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.title, Some("My Blog Post".to_string()));
        assert_eq!(meta.summary, Some("This is the summary".to_string()));
        assert_eq!(meta.date, Some("2025-01-15".to_string()));
        assert_eq!(meta.update, Some("2025-01-20".to_string()));
        assert_eq!(meta.author, Some("Alice".to_string()));
        assert!(!meta.draft);
    }

    #[test]
    fn test_content_meta_draft_default() {
        let json = r#"{"title": "Draft Test"}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert!(!meta.draft); // default is false
    }

    #[test]
    fn test_content_meta_draft_true() {
        let json = r#"{"title": "Draft", "draft": true}"#;
        let meta: ContentMeta = serde_json::from_str(json).unwrap();
        assert!(meta.draft);
    }
}
