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

use crate::{config::SiteConfig, generator::minify_xml, log, page::STORED_PAGES};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::fs;

const SITEMAP_NS: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";

/// Build sitemap if enabled.
pub fn build_sitemap(config: &SiteConfig) -> Result<()> {
    if config.build.sitemap.enable {
        let sitemap = Sitemap::build(config);
        sitemap.write(config)?;
    }
    Ok(())
}

struct Sitemap {
    urls: Vec<UrlEntry>,
}

struct UrlEntry {
    loc: String,
    lastmod: Option<String>,
}

impl Sitemap {
    fn build(config: &SiteConfig) -> Self {
        let pages = STORED_PAGES.get_pages();
        let base_url = config
            .site
            .info
            .url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');

        let urls: Vec<UrlEntry> = pages
            .iter()
            .map(|page| {
                let full_url = format!("{}{}", base_url, page.permalink.as_str());
                UrlEntry {
                    loc: full_url,
                    lastmod: page.meta.date.clone(),
                }
            })
            .collect();

        Self { urls }
    }

    fn into_xml(self) -> String {
        let mut xml = String::with_capacity(4096);

        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<urlset xmlns=\"");
        xml.push_str(SITEMAP_NS);
        xml.push_str("\">\n");

        for entry in self.urls {
            xml.push_str("  <url>\n    <loc>");
            xml.push_str(&escape_xml(&entry.loc));
            xml.push_str("</loc>\n");
            if let Some(lastmod) = entry.lastmod {
                xml.push_str("    <lastmod>");
                xml.push_str(&lastmod);
                xml.push_str("</lastmod>\n");
            }
            xml.push_str("  </url>\n");
        }

        xml.push_str("</urlset>\n");
        xml
    }

    fn write(self, config: &SiteConfig) -> Result<()> {
        // Resolve sitemap path relative to output_dir (with path_prefix)
        let sitemap_path = config.paths().output_dir().join(&config.build.sitemap.path);
        let xml = self.into_xml();
        let xml = minify_xml(xml.as_bytes(), config.build.minify);

        fs::write(&sitemap_path, &*xml)
            .with_context(|| format!("Failed to write sitemap to {}", sitemap_path.display()))?;

        log!("sitemap"; "{}", sitemap_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

/// Escape special XML characters.
fn escape_xml(s: &str) -> Cow<'_, str> {
    // Fast path: check if escaping is needed
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return Cow::Borrowed(s);
    }

    Cow::Owned(
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let sitemap = Sitemap { urls: vec![] };
        let xml = sitemap.into_xml();

        assert!(xml.contains(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains(&format!(r#"<urlset xmlns="{SITEMAP_NS}">"#)));
        assert!(xml.contains("</urlset>"));
        assert!(!xml.contains("<url>"));
    }

    #[test]
    fn test_sitemap_single_page() {
        let sitemap = Sitemap {
            urls: vec![UrlEntry {
                loc: "https://example.com/".to_string(),
                lastmod: Some("2025-01-01".to_string()),
            }],
        };
        let xml = sitemap.into_xml();

        assert!(xml.contains("<url>"));
        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(xml.contains("<lastmod>2025-01-01</lastmod>"));
        assert!(xml.contains("</url>"));
    }

    #[test]
    fn test_sitemap_multiple_pages() {
        let sitemap = Sitemap {
            urls: vec![
                UrlEntry {
                    loc: "https://example.com/".to_string(),
                    lastmod: Some("2025-01-01".to_string()),
                },
                UrlEntry {
                    loc: "https://example.com/posts/hello/".to_string(),
                    lastmod: Some("2025-01-02".to_string()),
                },
                UrlEntry {
                    loc: "https://example.com/about/".to_string(),
                    lastmod: None,
                },
            ],
        };
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(xml.contains("<loc>https://example.com/posts/hello/</loc>"));
        assert!(xml.contains("<loc>https://example.com/about/</loc>"));
        assert_eq!(xml.matches("<url>").count(), 3);
        assert_eq!(xml.matches("</url>").count(), 3);
    }

    #[test]
    fn test_sitemap_without_lastmod() {
        let sitemap = Sitemap {
            urls: vec![UrlEntry {
                loc: "https://example.com/".to_string(),
                lastmod: None,
            }],
        };
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/</loc>"));
        assert!(!xml.contains("<lastmod>"));
    }

    #[test]
    fn test_sitemap_escapes_special_chars() {
        let sitemap = Sitemap {
            urls: vec![UrlEntry {
                loc: "https://example.com/search?q=a&b=c".to_string(),
                lastmod: None,
            }],
        };
        let xml = sitemap.into_xml();

        assert!(xml.contains("<loc>https://example.com/search?q=a&amp;b=c</loc>"));
    }

    #[test]
    fn test_sitemap_xml_structure() {
        let sitemap = Sitemap {
            urls: vec![UrlEntry {
                loc: "https://example.com/".to_string(),
                lastmod: Some("2025-01-01".to_string()),
            }],
        };
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
