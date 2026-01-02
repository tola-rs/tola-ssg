//! Content generators for static site output.
//!
//! Generates auxiliary files from compiled page metadata:
//!
//! - **Feed**: RSS/Atom feeds for blog readers (`rss.xml`, `atom.xml`)
//! - **Sitemap**: Search engine indexing (`sitemap.xml`)
//!
//! Both generators use pre-collected `PageMeta` from the build pipeline,
//! avoiding redundant filesystem scans or re-compilation.

pub mod extract;
pub mod feed;
pub mod sitemap;

use std::borrow::Cow;

/// Minify XML content if enabled.
pub fn minify_xml(content: &[u8], enabled: bool) -> Cow<'_, [u8]> {
    if enabled {
        let xml_str = std::str::from_utf8(content).unwrap_or("");
        let minified = xml_str
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("");
        Cow::Owned(minified.into_bytes())
    } else {
        Cow::Borrowed(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minify_xml_basic() {
        let xml = br#"<?xml version="1.0"?>
<root>
  <item>Hello</item>
</root>"#;
        let result = minify_xml(xml, true);

        assert_eq!(
            &*result,
            br#"<?xml version="1.0"?><root><item>Hello</item></root>"#
        );
    }

    #[test]
    fn test_minify_xml_removes_indentation() {
        let xml = b"  <tag>  content  </tag>  ";
        let result = minify_xml(xml, true);

        assert_eq!(&*result, b"<tag>  content  </tag>");
    }

    #[test]
    fn test_minify_xml_removes_empty_lines() {
        let xml = b"<root>\n\n  <item/>\n\n</root>";
        let result = minify_xml(xml, true);

        assert_eq!(&*result, b"<root><item/></root>");
    }

    #[test]
    fn test_minify_xml_enabled() {
        let xml = b"<root>\n  <item/>\n</root>";

        let minified = minify_xml(xml, true);
        let not_minified = minify_xml(xml, false);

        assert_eq!(&*minified, b"<root><item/></root>");
        assert_eq!(&*not_minified, xml.as_slice());
    }
}
