//! Sitemap generation.
//!
//! Generates a sitemap.xml file listing all pages for search engine indexing.
//!
//! # Sitemap Format
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
//!   <url>
//!     <loc>https://example.com/</loc>
//!     <lastmod>2025-01-01</lastmod>
//!   </url>
//! </urlset>
//! ```

use crate::{compiler::meta::Pages, config::SiteConfig, log, utils::minify::{minify, MinifyType}};
use anyhow::{Context, Result};
use std::fs;

// ============================================================================
// Constants
// ============================================================================

/// XML namespace for sitemap
const SITEMAP_NS: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";

// ============================================================================
// Public API
// ============================================================================

/// Build sitemap if enabled in config.
///
/// Uses pre-collected page metadata instead of re-scanning the filesystem.
pub fn build_sitemap(config: &'static SiteConfig, pages: &Pages) -> Result<()> {
    if config.build.sitemap.enable {
        let sitemap = Sitemap::from_pages(pages);
        sitemap.write(config)?;
    }
    Ok(())
}

// ============================================================================
// Sitemap Implementation
// ============================================================================

/// Sitemap data structure
struct Sitemap {
    /// List of URL entries
    urls: Vec<UrlEntry>,
}

/// Single URL entry in the sitemap
struct UrlEntry {
    /// Full URL location
    loc: String,
    /// Last modification date (optional, YYYY-MM-DD format)
    lastmod: Option<String>,
}

impl Sitemap {
    /// Build sitemap from pre-collected page metadata.
    fn from_pages(pages: &Pages) -> Self {
        // log!("sitemap"; "generating from {} pages", pages.len());

        let urls: Vec<UrlEntry> = pages
            .iter()
            .map(|page| UrlEntry {
                loc: page.paths.full_url.clone(),
                lastmod: page.lastmod_ymd(),
            })
            .collect();

        Self { urls }
    }

    /// Generate sitemap XML string.
    fn into_xml(self) -> String {
        let mut xml = String::with_capacity(4096);

        xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        xml.push('\n');
        xml.push_str(&format!(r#"<urlset xmlns="{SITEMAP_NS}">"#));
        xml.push('\n');

        for entry in self.urls {
            xml.push_str("  <url>\n");
            xml.push_str(&format!("    <loc>{}</loc>\n", escape_xml(&entry.loc)));
            if let Some(lastmod) = entry.lastmod {
                xml.push_str(&format!("    <lastmod>{lastmod}</lastmod>\n"));
            }
            xml.push_str("  </url>\n");
        }

        xml.push_str("</urlset>\n");
        xml
    }

    /// Write sitemap to output file.
    fn write(self, config: &'static SiteConfig) -> Result<()> {
        let sitemap_path = &config.build.sitemap.path;
        let xml = self.into_xml();
        let xml = minify(MinifyType::Xml(xml.as_bytes()), config);

        fs::write(sitemap_path, &*xml)
            .with_context(|| format!("Failed to write sitemap to {}", sitemap_path.display()))?;

        log!("sitemap"; "{}", sitemap_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Escape special XML characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::meta::{PageMeta, PagePaths};
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn make_page(full_url: &str, lastmod_days: Option<u64>) -> PageMeta {
        PageMeta {
            paths: PagePaths {
                source: PathBuf::from("test.typ"),
                html: PathBuf::from("public/test/index.html"),
                relative: "test".to_string(),
                url_path: "/test/".to_string(),
                full_url: full_url.to_string(),
            },
            lastmod: lastmod_days.map(|days| UNIX_EPOCH + Duration::from_secs(days * 86400)),
            content_meta: None,
            compiled_html: None,
        }
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("hello"), "hello");
        assert_eq!(escape_xml("<test>"), "&lt;test&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(escape_xml("it's"), "it&apos;s");
    }

    #[test]
    fn test_escape_xml_combined() {
        assert_eq!(
            escape_xml("<a href=\"test\">link & 'text'</a>"),
            "&lt;a href=&quot;test&quot;&gt;link &amp; &apos;text&apos;&lt;/a&gt;"
        );
    }

    #[test]
    fn test_sitemap_empty() {
        let pages = Pages::default();
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        assert!(xml.contains(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains(&format!(r#"<urlset xmlns="{SITEMAP_NS}">"#)));
        assert!(xml.contains("</urlset>"));
        assert!(!xml.contains("<url>"));
    }

    #[test]
    fn test_sitemap_single_page() {
        let pages = Pages {
            items: vec![make_page("https://example.com/", Some(20089))], // 2025-01-01
        };
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        assert!(xml.contains("<url>"));
        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(xml.contains("<lastmod>2025-01-01</lastmod>"));
        assert!(xml.contains("</url>"));
    }

    #[test]
    fn test_sitemap_multiple_pages() {
        let pages = Pages {
            items: vec![
                make_page("https://example.com/", Some(20089)),
                make_page("https://example.com/posts/hello/", Some(20090)),
                make_page("https://example.com/about/", None),
            ],
        };
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(xml.contains("<loc>https://example.com/posts/hello/</loc>"));
        assert!(xml.contains("<loc>https://example.com/about/</loc>"));
        assert_eq!(xml.matches("<url>").count(), 3);
        assert_eq!(xml.matches("</url>").count(), 3);
    }

    #[test]
    fn test_sitemap_without_lastmod() {
        let pages = Pages {
            items: vec![make_page("https://example.com/", None)],
        };
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(!xml.contains("<lastmod>"));
    }

    #[test]
    fn test_sitemap_escapes_special_chars() {
        let pages = Pages {
            items: vec![make_page("https://example.com/search?q=a&b=c", None)],
        };
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/search?q=a&amp;b=c</loc>"));
    }

    #[test]
    fn test_sitemap_xml_structure() {
        let pages = Pages {
            items: vec![make_page("https://example.com/", Some(20089))],
        };
        let sitemap = Sitemap::from_pages(&pages);
        let xml = sitemap.into_xml();

        // Verify proper XML structure
        let lines: Vec<&str> = xml.lines().collect();
        assert_eq!(lines[0], r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        assert!(lines[1].starts_with("<urlset"));
        assert!(lines.last().unwrap().trim() == "</urlset>");
    }

    #[test]
    fn test_url_entry_with_lastmod() {
        let entry = UrlEntry {
            loc: "https://example.com/".to_string(),
            lastmod: Some("2025-01-01".to_string()),
        };

        assert_eq!(entry.loc, "https://example.com/");
        assert_eq!(entry.lastmod, Some("2025-01-01".to_string()));
    }

    #[test]
    fn test_url_entry_without_lastmod() {
        let entry = UrlEntry {
            loc: "https://example.com/".to_string(),
            lastmod: None,
        };

        assert_eq!(entry.loc, "https://example.com/");
        assert_eq!(entry.lastmod, None);
    }
}
