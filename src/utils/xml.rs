//! XML/HTML processing utilities.

use anyhow::Result;
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
};
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{Cursor, Write};
use std::str;

use crate::config::SiteConfig;
use crate::utils::meta::AssetMeta;
use crate::utils::slug::{slugify_fragment, slugify_path};
use crate::utils::svg::{HtmlContext, Svg, compress_svgs_parallel, extract_svg_element};
use std::path::Path;

// ============================================================================
// Type Aliases
// ============================================================================

pub type XmlWriter = Writer<Cursor<Vec<u8>>>;

// ============================================================================
// XML Reader
// ============================================================================

#[inline]
pub fn create_xml_reader(content: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(false);
    reader.config_mut().enable_all_checks(false);
    reader
}

// ============================================================================
// Element Builder
// ============================================================================

/// Rebuild an element with transformed attributes (avoids duplication bug).
fn rebuild_elem<F>(elem: &BytesStart<'_>, mut transform: F) -> BytesStart<'static>
where
    F: FnMut(&[u8], Cow<'_, [u8]>) -> Cow<'static, [u8]>,
{
    let tag = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let attrs: Vec<_> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key.as_ref().to_vec();
            let value = transform(attr.key.as_ref(), attr.value);
            (key, value)
        })
        .collect();

    let mut new_elem = BytesStart::new(tag);
    for (k, v) in attrs {
        new_elem.push_attribute((k.as_slice(), v.as_ref()));
    }
    new_elem
}

/// Rebuild an element with fallible attribute transformation.
fn rebuild_elem_try<F>(elem: &BytesStart<'_>, mut transform: F) -> Result<BytesStart<'static>>
where
    F: FnMut(&[u8], Cow<'_, [u8]>) -> Result<Cow<'static, [u8]>>,
{
    let tag = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let attrs: Result<Vec<_>> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key.as_ref().to_vec();
            let value = transform(attr.key.as_ref(), attr.value)?;
            Ok((key, value))
        })
        .collect();

    let mut new_elem = BytesStart::new(tag);
    for (k, v) in attrs? {
        new_elem.push_attribute((k.as_slice(), v.as_ref()));
    }
    Ok(new_elem)
}

// ============================================================================
// Element Writers
// ============================================================================

/// Write `<html>` element with `lang` attribute.
pub fn write_html_with_lang(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &SiteConfig,
) -> Result<()> {
    let mut elem = elem.to_owned();
    elem.push_attribute(("lang", config.base.language.as_str()));
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

/// Write heading element with slugified `id` attribute.
pub fn write_heading_with_slugified_id(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &'static SiteConfig,
) -> Result<()> {
    let new_elem = rebuild_elem(elem, |key, value| {
        if key == b"id" {
            let v = str::from_utf8(value.as_ref()).unwrap_or_default();
            slugify_fragment(v, config).into_bytes().into()
        } else {
            value.into_owned().into()
        }
    });
    writer.write_event(Event::Start(new_elem))?;
    Ok(())
}

/// Write element with processed `href` and `src` attributes.
pub fn write_element_with_processed_links(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &'static SiteConfig,
) -> Result<()> {
    let new_elem = rebuild_elem_try(elem, |key, value| {
        if matches!(key, b"href" | b"src") {
            process_link_value(&value, config)
        } else {
            Ok(value.into_owned().into())
        }
    })?;
    writer.write_event(Event::Start(new_elem))?;
    Ok(())
}

// ============================================================================
// Link Processing
// ============================================================================

/// Process a link value (href or src attribute).
///
/// # Link Type Detection
///
/// | Prefix | Type | Handler |
/// |--------|------|---------|
/// | `/` or `//` | Absolute | `process_absolute_link` |
/// | `#` | Fragment | `process_fragment_link` |
/// | `../` or `../../` | Relative | `process_relative_or_external_link` |
/// | `https://` | External | kept unchanged |
pub fn process_link_value(value: &[u8], config: &'static SiteConfig) -> Result<Cow<'static, [u8]>> {
    let value_str = str::from_utf8(value)?;
    let processed = match value_str.bytes().next() {
        Some(b'/') => process_absolute_link(value_str, config)?,
        Some(b'#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_or_external_link(value_str)?,
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
pub fn process_absolute_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let path_prefix = &config.build.path_prefix;

    if is_asset_link(value, config) {
        let value = value.trim_start_matches('/');
        return Ok(format!("/{}", path_prefix.join(value).display()));
    }

    let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
    let path = path.trim_start_matches('/');
    let slugified_path = slugify_path(path, config);

    let mut result = format!("/{}", path_prefix.join(&slugified_path).display());
    if !fragment.is_empty() {
        result.push('#');
        result.push_str(&slugify_fragment(fragment, config));
    }
    Ok(result)
}

/// Process fragment links (starting with `#`).
pub fn process_fragment_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    Ok(format!("#{}", slugify_fragment(&value[1..], config)))
}

/// Process relative or external links.
///
/// # Examples
///
/// | Input | Output |
/// |-------|--------|
/// | `../images/logo.png` | `../../images/logo.png` |
/// | `../../assets/doc.pdf` | `../../../assets/doc.pdf` |
/// | `other.html` | `../other.html` |
/// | `https://example.com` | `https://example.com` (unchanged) |
///
/// Note: Relative links get `../` prepended because content pages
/// are at `/post/index.html`, so need to go up one level first.
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

/// Write `<head>` section content before closing tag.
pub fn write_head_content(writer: &mut XmlWriter, config: &'static SiteConfig) -> Result<()> {
    let head = &config.build.head;

    if !config.base.title.is_empty() {
        write_text_element(writer, "title", &config.base.title)?;
    }
    if !config.base.description.is_empty() {
        write_empty_elem(
            writer,
            "meta",
            &[
                ("name", "description"),
                ("content", &config.base.description),
            ],
        )?;
    }

    if let Some(icon) = &head.icon {
        let href = compute_asset_href(icon, config)?;
        write_empty_elem(
            writer,
            "link",
            &[
                ("rel", "shortcut icon"),
                ("href", &href),
                ("type", get_icon_mime_type(icon)),
            ],
        )?;
    }

    for style in &head.styles {
        let href = compute_asset_href(style, config)?;
        write_empty_elem(writer, "link", &[("rel", "stylesheet"), ("href", &href)])?;
    }

    if config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
    {
        let href = compute_stylesheet_href(input, config)?;
        write_empty_elem(writer, "link", &[("rel", "stylesheet"), ("href", &href)])?;
    }

    // Scripts
    for script in &head.scripts {
        let src = compute_asset_href(script.path(), config)?;
        write_script(writer, &src, script.is_defer(), script.is_async())?;
    }

    // Raw HTML elements (trusted input)
    for raw in &head.elements {
        writer.get_mut().write_all(raw.as_bytes())?;
    }

    writer.write_event(Event::End(BytesEnd::new("head")))?;
    Ok(())
}

// ============================================================================
// Element Helpers
// ============================================================================

/// Write a text element: `<tag>text</tag>`.
#[inline]
pub fn write_text_element(writer: &mut XmlWriter, tag: &str, text: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

/// Write an empty element with attributes: `<tag attr1="val1" ... />`.
#[inline]
pub fn write_empty_elem(writer: &mut XmlWriter, tag: &str, attrs: &[(&str, &str)]) -> Result<()> {
    let mut elem = BytesStart::new(tag);
    for (k, v) in attrs {
        elem.push_attribute((*k, *v));
    }
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

/// Write a script element with optional defer/async.
pub fn write_script(
    writer: &mut XmlWriter,
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
// HTML Processing
// ============================================================================

pub fn process_html(html_path: &Path, content: &[u8], config: &'static SiteConfig) -> Result<Vec<u8>> {
    let mut ctx = HtmlContext::new(config, html_path);
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(content.len())));
    let mut reader = create_xml_reader(content);
    let mut svgs = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(elem)) => {
                handle_start_element(&elem, &mut reader, &mut writer, &mut ctx, &mut svgs)?;
            }
            Ok(Event::End(elem)) => {
                handle_end_element(&elem, &mut writer, config)?;
            }
            Ok(Event::Eof) => break,
            Ok(event) => writer.write_event(event)?,
            Err(e) => anyhow::bail!(
                "XML parse error at position {}: {:?}",
                reader.error_position(),
                e
            ),
        }
    }

    // Compress SVGs in parallel
    if ctx.extract_svg && !svgs.is_empty() {
        compress_svgs_parallel(&svgs, html_path, config)?;
    }

    Ok(writer.into_inner().into_inner())
}

fn handle_start_element(
    elem: &BytesStart<'_>,
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    ctx: &mut HtmlContext<'_>,
    svgs: &mut Vec<Svg>,
) -> Result<()> {
    match elem.name().as_ref() {
        b"html" => write_html_with_lang(elem, writer, ctx.config)?,
        b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
            write_heading_with_slugified_id(elem, writer, ctx.config)?;
        }
        b"svg" if ctx.extract_svg => {
            if let Some(svg) = extract_svg_element(reader, writer, elem, ctx)? {
                svgs.push(svg);
            }
        }
        _ => write_element_with_processed_links(elem, writer, ctx.config)?,
    }
    Ok(())
}

fn handle_end_element(
    elem: &BytesEnd<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    match elem.name().as_ref() {
        b"head" => write_head_content(writer, config)?,
        _ => writer.write_event(Event::End(elem.to_owned()))?,
    }
    Ok(())
}

// ============================================================================
// Asset Utilities
// ============================================================================

use std::ffi::OsString;
use std::fs;
use std::sync::OnceLock;

static ASSET_TOP_LEVELS: OnceLock<HashSet<OsString>> = OnceLock::new();

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

/// Compute href for an asset path relative to path_prefix
pub fn compute_asset_href(asset_path: &Path, config: &SiteConfig) -> Result<String> {
    let assets_dir = &config.build.assets;
    // Strip the leading "./" prefix if present
    let without_dot_prefix = asset_path.strip_prefix("./").unwrap_or(asset_path);
    // Strip the "assets/" prefix if present to get relative path within assets
    let relative_path = without_dot_prefix
        .strip_prefix("assets/")
        .unwrap_or(without_dot_prefix);

    let source = assets_dir.join(relative_path);
    let meta = AssetMeta::from_source(source, config)?;
    Ok(meta.url)
}

/// Compute stylesheet href from input path
pub fn compute_stylesheet_href(input: &Path, config: &SiteConfig) -> Result<String> {
    let input = input.canonicalize()?;
    let meta = AssetMeta::from_source(input, config)?;
    Ok(meta.url)
}

/// Get top-level asset directory names
fn get_asset_top_levels(assets_dir: &Path) -> &'static HashSet<OsString> {
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

    asset_top_levels.contains(first_component.as_ref() as &std::ffi::OsStr)
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
    use std::path::{Path, PathBuf};

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
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        let config = &config;

        let result = compute_asset_href(Path::new("images/icon.png"), config).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_prefix() {
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        let config = &config;

        let result = compute_asset_href(Path::new("./images/icon.png"), config).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_assets_prefix() {
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        let config = &config;

        let result =
            compute_asset_href(Path::new("assets/images/icon.png"), config).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_and_assets_prefix() {
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        let config = &config;

        let result =
            compute_asset_href(Path::new("./assets/images/icon.png"), config).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_path_prefix() {
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        config.build.path_prefix = PathBuf::from("blog");
        let config = &config;

        let result = compute_asset_href(Path::new("images/icon.png"), config).unwrap();
        assert_eq!(result, "/blog/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_full_path_with_prefix() {
        let mut config = SiteConfig::default();
        config.build.assets = PathBuf::from("/assets");
        config.build.output = PathBuf::from("/output");
        config.build.path_prefix = PathBuf::from("mysite");
        let config = &config;

        let result =
            compute_asset_href(Path::new("./assets/scripts/main.js"), config).unwrap();
        assert_eq!(result, "/mysite/scripts/main.js");
    }

    #[test]
    fn test_write_empty_elem_stylesheet() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_empty_elem(
            &mut writer,
            "link",
            &[("rel", "stylesheet"), ("href", "/styles/main.css")],
        )
        .unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("link"));
        assert!(output.contains("rel=\"stylesheet\""));
        assert!(output.contains("href=\"/styles/main.css\""));
    }

    #[test]
    fn test_write_script_basic() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script(&mut writer, "/scripts/main.js", false, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("<script"));
        assert!(output.contains("src=\"/scripts/main.js\""));
        assert!(output.contains("</script>"));
        assert!(!output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_with_defer() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script(&mut writer, "/scripts/main.js", true, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_with_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script(&mut writer, "/scripts/main.js", false, true).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(!output.contains("defer"));
        assert!(output.contains("async"));
    }

    #[test]
    fn test_write_script_with_both_defer_and_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script(&mut writer, "/scripts/main.js", true, true).unwrap();
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
