//! Page metadata collection and caching.
//!
//! `PageMeta` is the **primary metadata structure** for content pages,
//! containing all path and URL information needed across the build pipeline.
//!
//! # Architecture
//!
//! ```text
//! collect_pages() ──► Pages { items: Vec<PageMeta> }
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          │                   │                   │
//!          ▼                   ▼                   ▼
//!     build_site()       build_rss()        build_sitemap()
//!     (process each)     (query meta)       (use as-is)
//! ```
//!
//! # Usage
//!
//! ```ignore
//! let pages = collect_pages(config)?;
//!
//! for page in pages.iter() {
//!     // All path info available:
//!     // - page.source: original .typ file
//!     // - page.html: output HTML path
//!     // - page.relative: relative path (for logging)
//!     // - page.url_path: URL path with path_prefix
//!     // - page.full_url: complete URL with base
//!     // - page.lastmod_ymd(): formatted date for sitemap
//! }
//! ```

use crate::{
    config::SiteConfig,
    log,
    utils::slug::{slugify_path, remove_forbidden_chars},
};
use anyhow::{Result, anyhow};
use rayon::prelude::*;
use std::{fs, path::{Path, PathBuf}, time::SystemTime};

// ============================================================================
// Asset Metadata
// ============================================================================

/// Metadata for a static asset file.
///
/// Handles path resolution for assets, ensuring consistent URL generation
/// and output path calculation.
#[derive(Debug, Clone)]
pub struct AssetMeta {
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
    /// Create AssetMeta from a source path.
    pub fn from_source(source: PathBuf, config: &SiteConfig) -> Result<Self> {
        let assets_dir = &config.build.assets;
        let output_dir = config.build.output.join(&config.build.path_prefix);

        let relative = source
            .strip_prefix(assets_dir)
            .map_err(|_| anyhow!("File is not in assets directory: {}", source.display()))?
            .to_str()
            .ok_or_else(|| anyhow!("Invalid path encoding"))?
            .to_owned();

        let dest = output_dir.join(&relative);
        let url = url_from_output_path(&dest, config)?;

        Ok(Self {
            source,
            dest,
            relative,
            url,
        })
    }
}

/// Generate a URL path from an output file path.
///
/// Handles path prefix stripping and cross-platform separators.
pub fn url_from_output_path(path: &Path, config: &SiteConfig) -> Result<String> {
    let output_root = &config.build.output;

    // Strip output root
    let relative_to_output = path
        .strip_prefix(output_root)
        .map_err(|_| anyhow!("Path is not in output directory: {}", path.display()))?;

    // Convert to string and ensure forward slashes
    let path_str = relative_to_output.to_string_lossy().replace('\\', "/");

    // Ensure it starts with /
    let url = if path_str.starts_with('/') {
        path_str
    } else {
        format!("/{}", path_str)
    };

    Ok(url)
}

// ============================================================================
// Page Metadata
// ============================================================================

/// Primary metadata structure for a content page.
///
/// Contains all path and URL information needed by build, RSS and sitemap.
/// This is the **single source of truth** for page paths.
///
/// # Fields
///
/// | Field | Example | Used By |
/// |-------|---------|---------|
/// | `source` | `content/posts/hello.typ` | build, RSS query |
/// | `html` | `public/posts/hello/index.html` | build output |
/// | `relative` | `posts/hello` | logging |
/// | `url_path` | `/posts/hello/` | URL construction |
/// | `full_url` | `https://example.com/posts/hello/` | RSS, sitemap |
/// | `lastmod` | `SystemTime` | sitemap |
#[derive(Debug, Clone)]
pub struct PageMeta {
    /// Source .typ file path
    pub source: PathBuf,
    /// Generated HTML file path (includes path_prefix)
    pub html: PathBuf,
    /// Relative path without extension (for logging)
    pub relative: String,
    /// URL path component (includes path_prefix, e.g., `/prefix/posts/hello/`)
    #[allow(dead_code)] // Reserved for future use
    pub url_path: String,
    /// Full URL including base (e.g., `https://example.com/posts/hello/`)
    pub full_url: String,
    /// Last modification time of the HTML file
    pub lastmod: Option<SystemTime>,
}

impl PageMeta {
    /// Create PageMeta from a source .typ file path.
    ///
    /// Computes all derived paths:
    /// - `html`: output path with path_prefix
    /// - `relative`: for logging
    /// - `url_path`: URL path with path_prefix
    /// - `full_url`: complete URL with base
    /// - `lastmod`: from HTML file metadata
    pub fn from_source(source: PathBuf, config: &'static SiteConfig) -> Result<Self> {
        let content_dir = &config.build.content;
        let path_prefix = &config.build.path_prefix;
        let output_dir = config.build.output.join(path_prefix);
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
        let html = if is_root_index {
            output_dir.join("index.html")
        } else {
            output_dir.join(&relative).join("index.html")
        };
        let html = slugify_path(html, config);

        // Compute URL path
        let prefix_str = path_prefix.to_string_lossy();
        let prefix_str = prefix_str.trim_matches('/');

        let url_path = if is_root_index {
            if prefix_str.is_empty() {
                "/".to_string()
            } else {
                format!("/{}/", prefix_str)
            }
        } else if relative.ends_with("/index") {
            let rel = relative.trim_end_matches("/index");
            if prefix_str.is_empty() {
                format!("/{}/", rel)
            } else {
                format!("/{}/{}/", prefix_str, rel)
            }
        } else if prefix_str.is_empty() {
            format!("/{}/", relative)
        } else {
            format!("/{}/{}/", prefix_str, relative)
        };

        // Sanitize url_path (remove forbidden chars)
        let url_path = remove_forbidden_chars(&url_path);

        let full_url = format!("{}{}", base_url, url_path);
        let lastmod = fs::metadata(&html).and_then(|m| m.modified()).ok();

        Ok(Self {
            source,
            html,
            relative,
            url_path,
            full_url,
            lastmod,
        })
    }

    /// Get lastmod as YYYY-MM-DD string for sitemap.
    pub fn lastmod_ymd(&self) -> Option<String> {
        let modified = self.lastmod?;
        let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
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
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Collect page metadata from .typ files.
///
/// This is the **central entry point** for collecting page information.
pub fn collect_pages(config: &'static SiteConfig, typ_files: &[&PathBuf]) -> Pages {
    let items: Vec<PageMeta> = typ_files
        .par_iter()
        .filter_map(|typ_path| PageMeta::from_source((*typ_path).clone(), config).ok())
        .collect();

    log!("pages"; "collected {} pages", items.len());

    Pages { items }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert days since UNIX epoch (1970-01-01) to (year, month, day).
///
/// Uses Howard Hinnant's date algorithms for efficient calendar calculations.
/// See: <http://howardhinnant.github.io/date_algorithms.html>
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Shift epoch from 1970-01-01 to 0000-03-01
    let z = days + 719468;

    // Calculate era (400-year period)
    let era = if z >= 0 { z } else { z - 146096 } / 146097;

    // Day of era [0, 146096]
    let doe = (z - era * 146097) as u32;

    // Year of era [0, 399]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;

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
            source: PathBuf::from("test.typ"),
            html: PathBuf::from("public/test/index.html"),
            relative: "test".to_string(),
            url_path: "/test/".to_string(),
            full_url: "https://example.com/test/".to_string(),
            lastmod: Some(time),
        };

        let ymd = page.lastmod_ymd().unwrap();
        assert!(ymd.len() == 10);
        assert!(ymd.starts_with("2025-"));
    }

    #[test]
    fn test_lastmod_ymd_none() {
        let page = PageMeta {
            source: PathBuf::from("test.typ"),
            html: PathBuf::from("public/test/index.html"),
            relative: "test".to_string(),
            url_path: "/test/".to_string(),
            full_url: "https://example.com/test/".to_string(),
            lastmod: None,
        };

        assert_eq!(page.lastmod_ymd(), None);
    }

    #[test]
    fn test_page_meta_fields() {
        let page = PageMeta {
            source: PathBuf::from("content/posts/hello.typ"),
            html: PathBuf::from("public/posts/hello/index.html"),
            relative: "posts/hello".to_string(),
            url_path: "/posts/hello/".to_string(),
            full_url: "https://example.com/posts/hello/".to_string(),
            lastmod: None,
        };

        assert_eq!(page.source, PathBuf::from("content/posts/hello.typ"));
        assert_eq!(page.html, PathBuf::from("public/posts/hello/index.html"));
        assert_eq!(page.relative, "posts/hello");
        assert_eq!(page.url_path, "/posts/hello/");
        assert_eq!(page.full_url, "https://example.com/posts/hello/");
    }

    #[test]
    fn test_page_meta_root_index() {
        let page = PageMeta {
            source: PathBuf::from("content/index.typ"),
            html: PathBuf::from("public/index.html"),
            relative: "index".to_string(),
            url_path: "/".to_string(),
            full_url: "https://example.com/".to_string(),
            lastmod: None,
        };

        assert_eq!(page.relative, "index");
        assert_eq!(page.url_path, "/");
        assert_eq!(page.full_url, "https://example.com/");
    }

    #[test]
    fn test_page_meta_with_path_prefix() {
        let page = PageMeta {
            source: PathBuf::from("content/posts/hello.typ"),
            html: PathBuf::from("public/blog/posts/hello/index.html"),
            relative: "posts/hello".to_string(),
            url_path: "/blog/posts/hello/".to_string(),
            full_url: "https://example.com/blog/posts/hello/".to_string(),
            lastmod: None,
        };

        assert_eq!(
            page.html,
            PathBuf::from("public/blog/posts/hello/index.html")
        );
        assert_eq!(page.url_path, "/blog/posts/hello/");
        assert_eq!(page.full_url, "https://example.com/blog/posts/hello/");
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
                    source: PathBuf::from("a.typ"),
                    html: PathBuf::from("public/a/index.html"),
                    relative: "a".to_string(),
                    url_path: "/a/".to_string(),
                    full_url: "https://example.com/a/".to_string(),
                    lastmod: None,
                },
                PageMeta {
                    source: PathBuf::from("b.typ"),
                    html: PathBuf::from("public/b/index.html"),
                    relative: "b".to_string(),
                    url_path: "/b/".to_string(),
                    full_url: "https://example.com/b/".to_string(),
                    lastmod: None,
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
                    source: PathBuf::from("index.typ"),
                    html: PathBuf::from("public/index.html"),
                    relative: "index".to_string(),
                    url_path: "/".to_string(),
                    full_url: "https://example.com/".to_string(),
                    lastmod: None,
                },
                PageMeta {
                    source: PathBuf::from("posts/hello.typ"),
                    html: PathBuf::from("public/posts/hello/index.html"),
                    relative: "posts/hello".to_string(),
                    url_path: "/posts/hello/".to_string(),
                    full_url: "https://example.com/posts/hello/".to_string(),
                    lastmod: None,
                },
            ],
        };

        let urls: Vec<_> = pages.iter().map(|p| p.full_url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/", "https://example.com/posts/hello/"]
        );
    }
}
