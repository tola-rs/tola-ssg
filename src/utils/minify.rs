//! Minification utilities for HTML and XML.
//!
//! Provides a unified `minify` function that handles both HTML and XML,
//! with automatic enable/disable based on `SiteConfig`.

use crate::config::SiteConfig;
use std::borrow::Cow;

// ============================================================================
// Types
// ============================================================================

/// Content type for minification.
pub enum MinifyType<'a> {
    /// HTML content
    Html(&'a [u8]),
    /// XML content
    Xml(&'a [u8]),
}

// ============================================================================
// Unified Minify Function
// ============================================================================

/// Minify content based on type and config.
///
/// Returns `Cow::Borrowed` if minify disabled, `Cow::Owned` if minified.
pub fn minify<'a>(content: MinifyType<'a>, config: &SiteConfig) -> Cow<'a, [u8]> {
    if !config.build.minify {
        match content {
            MinifyType::Html(html) => Cow::Borrowed(html),
            MinifyType::Xml(xml) => Cow::Borrowed(xml),
        }
    } else {
        match content {
            MinifyType::Html(html) => Cow::Owned(minify_html_inner(html)),
            MinifyType::Xml(xml) => Cow::Owned(minify_xml_inner(xml)),
        }
    }
}

// ============================================================================
// Internal Implementation
// ============================================================================

/// Minify HTML content using `minify_html` crate.
fn minify_html_inner(html: &[u8]) -> Vec<u8> {
    let mut cfg = minify_html::Cfg::new();
    cfg.keep_closing_tags = true;
    cfg.keep_html_and_head_opening_tags = true;
    cfg.keep_comments = false;
    cfg.minify_css = true;
    cfg.minify_js = true;
    cfg.remove_bangs = true;
    cfg.remove_processing_instructions = true;
    minify_html::minify(html, &cfg)
}

/// Minify XML by removing unnecessary whitespace.
fn minify_xml_inner(xml: &[u8]) -> Vec<u8> {
    let xml_str = std::str::from_utf8(xml).unwrap_or("");
    xml_str
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("")
        .into_bytes()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfig;

    fn config_with_minify(enabled: bool) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.build.minify = enabled;
        config
    }

    // HTML minification tests

    #[test]
    fn test_minify_html_basic() {
        let html = b"<html>\n  <head>\n  </head>\n  <body>\n    <p>Hello</p>\n  </body>\n</html>";
        let config = config_with_minify(true);
        let result = minify(MinifyType::Html(html), &config);
        let result_str = String::from_utf8_lossy(&result);

        // Should remove unnecessary whitespace
        assert!(!result_str.contains("\n  "));
        assert!(result_str.contains("<p>Hello</p>"));
    }

    #[test]
    fn test_minify_html_preserves_content() {
        let html = b"<p>Hello World</p>";
        let config = config_with_minify(true);
        let result = minify(MinifyType::Html(html), &config);
        let result_str = String::from_utf8_lossy(&result);

        assert!(result_str.contains("Hello World"));
    }

    #[test]
    fn test_minify_html_enabled() {
        let html = b"<html>\n  <body>\n  </body>\n</html>";

        let minified = minify(MinifyType::Html(html), &config_with_minify(true));
        let not_minified = minify(MinifyType::Html(html), &config_with_minify(false));

        assert!(minified.len() < not_minified.len());
        assert_eq!(&*not_minified, html);
    }

    #[test]
    fn test_minify_html_disabled() {
        let html = b"<html>\n  <body>\n  </body>\n</html>";
        let result = minify(MinifyType::Html(html), &config_with_minify(false));

        assert_eq!(&*result, html);
    }

    // XML minification tests

    #[test]
    fn test_minify_xml_basic() {
        let xml = br#"<?xml version="1.0"?>
<root>
  <item>Hello</item>
</root>"#;
        let result = minify(MinifyType::Xml(xml), &config_with_minify(true));

        assert_eq!(
            &*result,
            br#"<?xml version="1.0"?><root><item>Hello</item></root>"#
        );
    }

    #[test]
    fn test_minify_xml_removes_indentation() {
        let xml = b"  <tag>  content  </tag>  ";
        let result = minify(MinifyType::Xml(xml), &config_with_minify(true));

        assert_eq!(&*result, b"<tag>  content  </tag>");
    }

    #[test]
    fn test_minify_xml_removes_empty_lines() {
        let xml = b"<root>\n\n  <item/>\n\n</root>";
        let result = minify(MinifyType::Xml(xml), &config_with_minify(true));

        assert_eq!(&*result, b"<root><item/></root>");
    }

    #[test]
    fn test_minify_xml_enabled() {
        let xml = b"<root>\n  <item/>\n</root>";

        let minified = minify(MinifyType::Xml(xml), &config_with_minify(true));
        let not_minified = minify(MinifyType::Xml(xml), &config_with_minify(false));

        assert_eq!(&*minified, b"<root><item/></root>");
        assert_eq!(&*not_minified, xml.as_slice());
    }

    #[test]
    fn test_minify_xml_sitemap_like() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/</loc>
    <lastmod>2025-01-01</lastmod>
  </url>
</urlset>"#;
        let result = minify(MinifyType::Xml(xml), &config_with_minify(true));
        let result_str = String::from_utf8_lossy(&result);

        assert!(!result_str.contains('\n'));
        assert!(!result_str.contains("  "));
        assert!(result_str.contains("<loc>https://example.com/</loc>"));
        assert!(result_str.contains("<lastmod>2025-01-01</lastmod>"));
    }

    #[test]
    fn test_minify_xml_rss_like() {
        let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <title>Test</title>
    <item>
      <title>Post</title>
    </item>
  </channel>
</rss>"#;
        let result = minify(MinifyType::Xml(xml), &config_with_minify(true));
        let result_str = String::from_utf8_lossy(&result);

        assert!(!result_str.contains('\n'));
        assert!(result_str.contains("<title>Test</title>"));
        assert!(result_str.contains("<title>Post</title>"));
    }
}
