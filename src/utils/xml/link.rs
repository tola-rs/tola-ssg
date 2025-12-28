use crate::config::SiteConfig;
use crate::utils::slug::{slugify_fragment, slugify_path};
use anyhow::Result;
use std::borrow::Cow;
use std::str;

use super::assets::is_asset_link;

/// Process a link value (href or src attribute).
///
/// # Link Type Detection
///
/// | Prefix | Type | Handler |
/// |--------|------|---------|
/// | `/` or `//` | Absolute | `process_absolute_link` |
/// | `#` | Fragment | `process_fragment_link` |
/// | `../` or `./` | Relative | `process_relative_link` |
/// | `https://` | External | kept unchanged |
///
/// # Arguments
///
/// * `is_source_index` - Whether the source was `index.typ`. Affects relative path resolution:
///   - `index.typ` → `dir/index.html` (same level, no adjustment)
///   - `foo.typ` → `foo/index.html` (one level deeper, needs `../` prefix)
pub fn process_link_value(
    value: &[u8],
    config: &SiteConfig,
    is_source_index: bool,
) -> Result<Cow<'static, [u8]>> {
    let value_str = str::from_utf8(value)?;
    let processed = match value_str.bytes().next() {
        Some(b'/') => process_absolute_link(value_str, config)?,
        Some(b'#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_link(value_str, is_source_index)?,
        None => anyhow::bail!("empty link URL found in typst file"),
    };
    Ok(Cow::Owned(processed.into_bytes()))
}

/// Process absolute links (starting with `/` or `//`).
///
/// # Examples
///
/// | Input | Output (path_prefix="") |
/// |-------|----------------------|
/// | `/about` | `/about` |
/// | `/about#team` | `/about#team` (fragment slugified) |
/// | `//example.com` | `//example.com` (protocol-relative) |
#[allow(clippy::unnecessary_wraps)] // Result for API consistency
pub fn process_absolute_link(value: &str, config: &SiteConfig) -> Result<String> {
    let paths = config.paths();

    if is_asset_link(value, config) {
        let value = value.trim_start_matches('/');
        return Ok(paths.url_for_rel_path(value));
    }

    let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
    let path = path.trim_start_matches('/');
    let slugified_path = slugify_path(path, config);

    let mut result = paths.url_for_rel_path(&slugified_path);
    if !fragment.is_empty() {
        result.push('#');
        result.push_str(&slugify_fragment(fragment, config));
    }
    Ok(result)
}

/// Process fragment links (starting with `#`).
#[allow(clippy::unnecessary_wraps)] // Result for API consistency
pub fn process_fragment_link(value: &str, config: &SiteConfig) -> Result<String> {
    Ok(format!("#{}", slugify_fragment(&value[1..], config)))
}

/// Process relative links (starting with `./`, `../`, or no prefix).
///
/// # Path Adjustment Logic
///
/// | Source File | Output Structure | Relative Path Adjustment |
/// |-------------|------------------|--------------------------|
/// | `foo/index.typ` | `foo/index.html` | None (same level) |
/// | `foo.typ` | `foo/index.html` | Prepend `../` (one level deeper) |
///
/// # Examples
///
/// For `index.typ`:
/// - `./img.png` → `./img.png` (no change)
/// - `../doc.pdf` → `../doc.pdf` (no change)
///
/// For `page.typ`:
/// - `./img.png` → `../img.png` (adjusted for extra directory level)
/// - `../doc.pdf` → `../../doc.pdf` (adjusted)
#[allow(clippy::unnecessary_wraps)] // Result for API consistency
pub fn process_relative_link(value: &str, is_source_index: bool) -> Result<String> {
    Ok(if is_external_link(value) {
        // External links (http:, mailto:, etc.) are unchanged
        value.to_string()
    } else if is_source_index {
        // index.typ: output is at same level, no adjustment needed
        value.to_string()
    } else {
        // Non-index files: output is one level deeper, prepend ../
        format!("../{value}")
    })
}

/// Check if a link is external (has a scheme like http:, mailto:, etc.)
///
/// A valid scheme must:
/// - Have at least 1 character before the colon
/// - Only contain ASCII alphanumeric or `+`, `-`, `.`
#[inline]
pub fn is_external_link(link: &str) -> bool {
    link.find(':').is_some_and(|pos| {
        // Scheme must be non-empty
        pos > 0 && link[..pos]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // process_relative_link tests
    // ========================================================================

    #[test]
    fn test_relative_link_index_no_adjustment() {
        // index.typ: output is at same level, no adjustment needed
        assert_eq!(process_relative_link("./img.png", true).unwrap(), "./img.png");
        assert_eq!(process_relative_link("../doc.pdf", true).unwrap(), "../doc.pdf");
        assert_eq!(process_relative_link("asset/logo.svg", true).unwrap(), "asset/logo.svg");
        assert_eq!(process_relative_link("../../up/up.txt", true).unwrap(), "../../up/up.txt");
    }

    #[test]
    fn test_relative_link_non_index_prepend() {
        // Non-index.typ: output is one level deeper, prepend ../
        assert_eq!(process_relative_link("./img.png", false).unwrap(), ".././img.png");
        assert_eq!(process_relative_link("../doc.pdf", false).unwrap(), "../../doc.pdf");
        assert_eq!(process_relative_link("asset/logo.svg", false).unwrap(), "../asset/logo.svg");
    }

    #[test]
    fn test_relative_link_external_unchanged() {
        // External links: unchanged regardless of is_source_index
        assert_eq!(process_relative_link("https://example.com", true).unwrap(), "https://example.com");
        assert_eq!(process_relative_link("https://example.com", false).unwrap(), "https://example.com");
        assert_eq!(process_relative_link("mailto:user@example.com", true).unwrap(), "mailto:user@example.com");
        assert_eq!(process_relative_link("tel:+1234567890", false).unwrap(), "tel:+1234567890");
    }

    // ========================================================================
    // is_external_link tests
    // ========================================================================

    #[test]
    fn test_is_external_link_schemes() {
        // Common protocols
        assert!(is_external_link("https://example.com"));
        assert!(is_external_link("http://example.com"));
        assert!(is_external_link("mailto:user@example.com"));
        assert!(is_external_link("tel:+1234567890"));
        assert!(is_external_link("ftp://files.example.com"));
        assert!(is_external_link("data:text/html,hello"));
    }

    #[test]
    fn test_is_external_link_not_external() {
        // Relative paths and fragments
        assert!(!is_external_link("./img.png"));
        assert!(!is_external_link("../doc.pdf"));
        assert!(!is_external_link("/about"));
        assert!(!is_external_link("#section"));
        assert!(!is_external_link("path/to/file"));
    }

    #[test]
    fn test_is_external_link_edge_cases() {
        // Edge cases
        assert!(is_external_link("file:with:multiple:colons")); // file: is valid scheme
        assert!(is_external_link("custom+scheme://example")); // custom scheme with +
        assert!(!is_external_link(":invalid")); // no scheme before colon
        assert!(!is_external_link("")); // empty string
        assert!(!is_external_link("中文:path")); // non-ASCII before colon is not valid scheme
    }

    // ========================================================================
    // process_fragment_link tests
    // ========================================================================

    #[test]
    fn test_fragment_link_simple() {
        let config = SiteConfig::default();
        assert_eq!(process_fragment_link("#section", &config).unwrap(), "#section");
        assert_eq!(process_fragment_link("#my-heading", &config).unwrap(), "#my-heading");
    }

    #[test]
    fn test_fragment_link_with_spaces() {
        let config = SiteConfig::default();
        // Fragment gets slugified
        let result = process_fragment_link("#My Section", &config).unwrap();
        assert!(result.starts_with('#'));
    }

    // ========================================================================
    // process_link_value integration tests
    // ========================================================================

    #[test]
    fn test_process_link_value_dispatch() {
        let config = SiteConfig::default();

        // Absolute path -> process_absolute_link
        let result = process_link_value(b"/about", &config, true).unwrap();
        assert!(result.starts_with(b"/"));

        // Fragment -> process_fragment_link
        let result = process_link_value(b"#section", &config, true).unwrap();
        assert!(result.starts_with(b"#"));

        // External link -> unchanged
        let result = process_link_value(b"https://example.com", &config, true).unwrap();
        assert_eq!(&*result, b"https://example.com");

        // Relative path (index.typ) -> no adjustment
        let result = process_link_value(b"./img.png", &config, true).unwrap();
        assert_eq!(&*result, b"./img.png");

        // Relative path (non-index.typ) -> prepend ../
        let result = process_link_value(b"./img.png", &config, false).unwrap();
        assert_eq!(&*result, b".././img.png");
    }

    #[test]
    fn test_process_link_value_empty_error() {
        let config = SiteConfig::default();
        let result = process_link_value(b"", &config, true);
        assert!(result.is_err());
    }
}
