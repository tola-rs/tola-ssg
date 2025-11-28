//! XML processing utilities.
//!
//! Provides XML reader creation and element processing helpers.

use anyhow::Result;
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event, attributes::Attribute},
};
use std::borrow::Cow;
use std::io::{Cursor, Write};
use std::str;

use crate::config::SiteConfig;
use crate::utils::slug::{slugify_fragment, slugify_path};

// ============================================================================
// XML Reader Creation
// ============================================================================

/// Create a configured XML reader from content bytes
#[inline]
pub fn create_xml_reader(content: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(false);
    reader.config_mut().enable_all_checks(false);
    reader
}

// ============================================================================
// Element Writers
// ============================================================================

/// Write HTML element with lang attribute
pub fn write_html_with_lang(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &SiteConfig,
) -> Result<()> {
    let mut elem = elem.to_owned();
    elem.push_attribute(("lang", config.base.language.as_str()));
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

/// Write heading element with slugified id attribute
pub fn write_heading_with_slugified_id(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let attrs: Vec<Attribute> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key;
            let value = if key.as_ref() == b"id" {
                let v = str::from_utf8(attr.value.as_ref()).unwrap_or_default();
                slugify_fragment(v, config).into_bytes().into()
            } else {
                attr.value
            };
            Attribute { key, value }
        })
        .collect();

    let elem = elem.to_owned().with_attributes(attrs);
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

/// Write element with processed href/src links
pub fn write_element_with_processed_links(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let attrs: Result<Vec<Attribute>> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key;
            let value = if key.as_ref() == b"href" || key.as_ref() == b"src" {
                process_link_value(&attr.value, config)?
            } else {
                attr.value
            };
            Ok(Attribute { key, value })
        })
        .collect();

    let elem = elem.to_owned().with_attributes(attrs?);
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

// ============================================================================
// Link Processing
// ============================================================================

/// Process a link value (href or src attribute)
pub fn process_link_value<'a>(
    value: &Cow<'a, [u8]>,
    config: &'static SiteConfig,
) -> Result<Cow<'a, [u8]>> {
    let value_str = str::from_utf8(value.as_ref())?;
    let processed = match value_str.bytes().next() {
        Some(b'/') => process_absolute_link(value_str, config)?,
        Some(b'#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_or_external_link(value_str)?,
        None => anyhow::bail!("empty link URL found in typst file"),
    };
    Ok(processed.into_bytes().into())
}

/// Process absolute links (starting with /)
pub fn process_absolute_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let base_path = &config.build.base_path;

    if is_asset_link(value, config) {
        let value = value.trim_start_matches('/');
        return Ok(format!("/{}", base_path.join(value).display()));
    }

    let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
    let path = path.trim_start_matches('/');
    let slugified_path = slugify_path(path, config);

    let mut result = format!("/{}", base_path.join(&slugified_path).display());
    if !fragment.is_empty() {
        result.push('#');
        result.push_str(&slugify_fragment(fragment, config));
    }
    Ok(result)
}

/// Process fragment links (starting with #)
pub fn process_fragment_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    Ok(format!("#{}", slugify_fragment(&value[1..], config)))
}

/// Process relative or external links
pub fn process_relative_or_external_link(value: &str) -> Result<String> {
    Ok(if is_external_link(value) {
        value.to_string()
    } else {
        format!("../{value}")
    })
}

// ============================================================================
// Head Section Processing
// ============================================================================

/// Write head section content (title, meta, links, scripts)
pub fn write_head_content(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let head = &config.build.head;
    let base_path = &config.build.base_path;

    // Title
    if !config.base.title.is_empty() {
        write_text_element(writer, "title", &config.base.title)?;
    }

    // Description meta tag
    if !config.base.description.is_empty() {
        write_meta_tag(writer, "description", &config.base.description)?;
    }

    // Favicon
    if let Some(icon) = &head.icon {
        write_icon_link(writer, icon, base_path)?;
    }

    // Stylesheets
    for style in &head.styles {
        let href = compute_asset_href(style, base_path)?;
        write_stylesheet_link(writer, &href)?;
    }

    // Tailwind stylesheet
    if config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
    {
        let href = compute_stylesheet_href(input, config)?;
        write_stylesheet_link(writer, &href)?;
    }

    // Scripts
    for script in &head.scripts {
        let src = compute_asset_href(script.path(), base_path)?;
        write_script_element(writer, &src, script.is_defer(), script.is_async())?;
    }

    // Raw HTML elements (trusted input)
    for raw in &head.elements {
        writer.get_mut().write_all(raw.as_bytes())?;
    }

    writer.write_event(Event::End(BytesEnd::new("head")))?;
    Ok(())
}

/// Write a simple text element (e.g., <title>text</title>)
#[inline]
pub fn write_text_element(writer: &mut Writer<Cursor<Vec<u8>>>, tag: &str, text: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

/// Write a meta tag
#[inline]
pub fn write_meta_tag(writer: &mut Writer<Cursor<Vec<u8>>>, name: &str, content: &str) -> Result<()> {
    let mut elem = BytesStart::new("meta");
    elem.push_attribute(("name", name));
    elem.push_attribute(("content", content));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

/// Write an icon link element
#[inline]
pub fn write_icon_link(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    icon: &std::path::Path,
    base_path: &std::path::Path,
) -> Result<()> {
    let href = compute_asset_href(icon, base_path)?;
    let mime_type = get_icon_mime_type(icon);

    let mut elem = BytesStart::new("link");
    elem.push_attribute(("rel", "shortcut icon"));
    elem.push_attribute(("href", href.as_str()));
    elem.push_attribute(("type", mime_type));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

/// Write a stylesheet link element
#[inline]
pub fn write_stylesheet_link(writer: &mut Writer<Cursor<Vec<u8>>>, href: &str) -> Result<()> {
    let mut elem = BytesStart::new("link");
    elem.push_attribute(("rel", "stylesheet"));
    elem.push_attribute(("href", href));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

/// Write a script element
pub fn write_script_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    src: &str,
    defer: bool,
    async_attr: bool,
) -> Result<()> {
    let mut elem = BytesStart::new("script");
    elem.push_attribute(("src", src));
    if defer {
        elem.push_attribute(("defer", ""));
    }
    if async_attr {
        elem.push_attribute(("async", ""));
    }
    writer.write_event(Event::Start(elem))?;
    // Space ensures proper HTML parsing of script tags
    writer.write_event(Event::Text(BytesText::new(" ")))?;
    writer.write_event(Event::End(BytesEnd::new("script")))?;
    Ok(())
}

// ============================================================================
// Asset Utilities
// ============================================================================

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ASSET_TOP_LEVELS: OnceLock<Vec<OsString>> = OnceLock::new();

/// Get MIME type for icon based on file extension
pub fn get_icon_mime_type(path: &Path) -> &'static str {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "ico" => "image/x-icon",
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "avif" => "image/avif",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "jpg" | "jpeg" => "image/jpeg",
            _ => "image/x-icon",
        })
        .unwrap_or("image/x-icon")
}

/// Compute href for an asset path relative to base_path
pub fn compute_asset_href(asset_path: &Path, base_path: &Path) -> Result<String> {
    // Strip the leading "./" prefix if present
    let without_dot_prefix = asset_path.strip_prefix("./").unwrap_or(asset_path);
    // Strip the "assets/" prefix if present to get relative path within assets
    let relative_path = without_dot_prefix
        .strip_prefix("assets/")
        .unwrap_or(without_dot_prefix);
    let path = PathBuf::from("/").join(base_path).join(relative_path);
    Ok(path.to_string_lossy().into_owned())
}

/// Compute stylesheet href from input path
pub fn compute_stylesheet_href(input: &Path, config: &'static SiteConfig) -> Result<String> {
    let base_path = &config.build.base_path;
    // Config assets path is already absolute
    let assets = &config.build.assets;
    let input = input.canonicalize()?;
    let relative = input.strip_prefix(assets)?;
    let path = PathBuf::from("/").join(base_path).join(relative);
    Ok(path.to_string_lossy().into_owned())
}

/// Get top-level asset directory names
pub fn get_asset_top_levels(assets_dir: &Path) -> &'static [OsString] {
    ASSET_TOP_LEVELS.get_or_init(|| {
        fs::read_dir(assets_dir)
            .map(|dir| dir.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default()
    })
}

/// Check if a path is an asset link
pub fn is_asset_link(path: &str, config: &'static SiteConfig) -> bool {
    let asset_top_levels = get_asset_top_levels(&config.build.assets);

    // Extract first path component after the leading slash
    let first_component = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();

    asset_top_levels.iter().any(|name| name == first_component)
}

/// Check if a link is external (has a scheme like http:, mailto:, etc.)
#[inline]
pub fn is_external_link(link: &str) -> bool {
    link.find(':').is_some_and(|pos| {
        link[..pos]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_get_icon_mime_type_ico() {
        assert_eq!(get_icon_mime_type(Path::new("favicon.ico")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_png() {
        assert_eq!(get_icon_mime_type(Path::new("icon.png")), "image/png");
    }

    #[test]
    fn test_get_icon_mime_type_svg() {
        assert_eq!(get_icon_mime_type(Path::new("logo.svg")), "image/svg+xml");
    }

    #[test]
    fn test_get_icon_mime_type_avif() {
        assert_eq!(get_icon_mime_type(Path::new("image.avif")), "image/avif");
    }

    #[test]
    fn test_get_icon_mime_type_webp() {
        assert_eq!(get_icon_mime_type(Path::new("photo.webp")), "image/webp");
    }

    #[test]
    fn test_get_icon_mime_type_gif() {
        assert_eq!(get_icon_mime_type(Path::new("animation.gif")), "image/gif");
    }

    #[test]
    fn test_get_icon_mime_type_jpeg() {
        assert_eq!(get_icon_mime_type(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(get_icon_mime_type(Path::new("photo.jpeg")), "image/jpeg");
    }

    #[test]
    fn test_get_icon_mime_type_unknown_defaults_to_ico() {
        assert_eq!(get_icon_mime_type(Path::new("file.xyz")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_no_extension_defaults_to_ico() {
        assert_eq!(get_icon_mime_type(Path::new("favicon")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_case_insensitive() {
        assert_eq!(get_icon_mime_type(Path::new("icon.PNG")), "image/png");
        assert_eq!(get_icon_mime_type(Path::new("logo.SVG")), "image/svg+xml");
        assert_eq!(get_icon_mime_type(Path::new("photo.JPEG")), "image/jpeg");
    }

    #[test]
    fn test_compute_asset_href_simple_path() {
        let result = compute_asset_href(Path::new("images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_prefix() {
        let result = compute_asset_href(Path::new("./images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_assets_prefix() {
        let result =
            compute_asset_href(Path::new("assets/images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_and_assets_prefix() {
        let result =
            compute_asset_href(Path::new("./assets/images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_base_path() {
        let result = compute_asset_href(Path::new("images/icon.png"), Path::new("blog")).unwrap();
        assert_eq!(result, "/blog/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_full_path_with_base() {
        let result =
            compute_asset_href(Path::new("./assets/scripts/main.js"), Path::new("mysite")).unwrap();
        assert_eq!(result, "/mysite/scripts/main.js");
    }

    #[test]
    fn test_write_stylesheet_link() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_stylesheet_link(&mut writer, "/styles/main.css").unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("link"));
        assert!(output.contains("rel=\"stylesheet\""));
        assert!(output.contains("href=\"/styles/main.css\""));
    }

    #[test]
    fn test_write_script_element_basic() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", false, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("<script"));
        assert!(output.contains("src=\"/scripts/main.js\""));
        assert!(output.contains("</script>"));
        assert!(!output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_defer() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", true, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", false, true).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(!output.contains("defer"));
        assert!(output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_both_defer_and_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", true, true).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("defer"));
        assert!(output.contains("async"));
    }

    #[test]
    fn test_is_external_link_http() {
        assert!(is_external_link("http://example.com"));
        assert!(is_external_link("https://example.com/path"));
    }

    #[test]
    fn test_is_external_link_mailto() {
        assert!(is_external_link("mailto:user@example.com"));
    }

    #[test]
    fn test_is_external_link_relative_path() {
        assert!(!is_external_link("/path/to/page"));
        assert!(!is_external_link("./relative/path"));
        assert!(!is_external_link("../parent/path"));
    }

    #[test]
    fn test_is_external_link_anchor() {
        assert!(!is_external_link("#section"));
    }

    #[test]
    fn test_process_link_value() {
        let config = Box::leak(Box::new(SiteConfig::default()));

        // Absolute link
        let value = Cow::Borrowed(b"/about".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "/about");

        // Fragment link
        let value = Cow::Borrowed(b"#header".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "#header");

        // Relative link
        let value = Cow::Borrowed(b"contact".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "../contact");

        // Absolute link with fragment
        let value = Cow::Borrowed(b"/about#team".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "/about#team");

        // Relative link with fragment
        let value = Cow::Borrowed(b"contact#form".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "../contact#form");

        // Relative link with parent directory
        let value = Cow::Borrowed(b"../images/logo.png".as_slice());
        let result = process_link_value(&value, config).unwrap();
        assert_eq!(String::from_utf8_lossy(&result), "../../images/logo.png");
    }
}
