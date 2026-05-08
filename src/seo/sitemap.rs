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

use crate::{config::SiteConfig, log, page::StoredPageMap, seo::minify_xml};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::fs;

const SITEMAP_NS: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";

/// Build sitemap if enabled
pub fn build_sitemap(config: &SiteConfig, store: &StoredPageMap) -> Result<()> {
    if config.site.seo.sitemap.enable {
        let sitemap = Sitemap::build(config, store);
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
    fn build(config: &SiteConfig, store: &StoredPageMap) -> Self {
        let pages = store.get_pages();

        let urls: Vec<UrlEntry> = pages
            .iter()
            .map(|page| {
                let full_url = page
                    .permalink
                    .canonical_url(config.site.info.url.as_deref());
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
        let sitemap_path = config
            .paths()
            .output_dir()
            .join(&config.site.seo.sitemap.path);
        let xml = self.into_xml();
        let xml = minify_xml(xml.as_bytes(), config.build.minify);

        fs::write(&sitemap_path, &*xml)
            .with_context(|| format!("Failed to write sitemap to {}", sitemap_path.display()))?;

        log!("sitemap"; "{}", sitemap_path.file_name().unwrap_or_default().to_string_lossy());
        Ok(())
    }
}

/// Escape special XML characters
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
    fn test_escape_xml_cases() {
        for (input, expected) in [
            ("hello", "hello"),
            ("<test>", "&lt;test&gt;"),
            ("a & b", "a &amp; b"),
            (r#"say "hi""#, "say &quot;hi&quot;"),
            ("it's", "it&apos;s"),
            (
                "<a href=\"test\">link & 'text'</a>",
                "&lt;a href=&quot;test&quot;&gt;link &amp; &apos;text&apos;&lt;/a&gt;",
            ),
        ] {
            assert_eq!(escape_xml(input), expected, "{input:?}");
        }
    }

    #[test]
    fn test_sitemap_empty() {
        let sitemap = Sitemap { urls: vec![] };
        let xml = sitemap.into_xml();
        assert_eq!(
            xml,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="{SITEMAP_NS}">
</urlset>
"#
            )
        );
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
        assert_eq!(
            xml,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="{SITEMAP_NS}">
  <url>
    <loc>https://example.com/search?q=a&amp;b=c</loc>
  </url>
</urlset>
"#
            )
        );
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
}
