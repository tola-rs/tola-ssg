//! SVG extraction, optimization, and compression.
//!
//! This module handles:
//! - Extracting SVG elements from Typst-generated HTML
//! - Optimizing SVGs using usvg
//! - Compressing to AVIF using various backends (builtin, magick, ffmpeg)

use anyhow::{Context, Result};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use rayon::prelude::*;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::time::SystemTime;

use crate::config::{ExtractSvgType, SiteConfig};
use crate::utils::meta::url_from_output_path;
use crate::{exec_with_stdin, log};

// ============================================================================
// Constants
// ============================================================================

/// Padding adjustments for SVG viewBox (typst HTML output quirks)
const SVG_PADDING_TOP: f32 = 5.0;
const SVG_PADDING_BOTTOM: f32 = 4.0;

/// Initial buffer size for capturing SVG content (16KB)
const INITIAL_SVG_BUFFER_SIZE: usize = 16 * 1024;

// ============================================================================
// Core Types
// ============================================================================

/// Extracted SVG with optimized data and metadata
pub struct Svg {
    /// Optimized SVG content
    pub data: Vec<u8>,
    /// Dimensions (width, height) in pixels
    pub size: (f32, f32),
    /// Sequential index for naming
    pub index: usize,
}

impl Svg {
    /// Create new SVG with the given data, size, and index
    #[inline]
    pub const fn new(data: Vec<u8>, size: (f32, f32), index: usize) -> Self {
        Self { data, size, index }
    }

    /// Determine the output format based on config and file size
    #[inline]
    fn output_format(&self, config: &SiteConfig) -> OutputFormat {
        if matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || self.data.len() < config.get_inline_max_size()
        {
            OutputFormat::Svg
        } else {
            OutputFormat::Avif
        }
    }

    /// Generate output filename (e.g., "svg-0.svg" or "svg-0.avif")
    #[inline]
    pub fn filename(&self, config: &SiteConfig) -> String {
        match self.output_format(config) {
            OutputFormat::Svg => format!("svg-{}.svg", self.index),
            OutputFormat::Avif => format!("svg-{}.avif", self.index),
        }
    }
}

/// Output format for extracted SVGs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Svg,
    Avif,
}

/// Processing context for HTML transformation
pub struct HtmlContext<'a> {
    pub config: &'static SiteConfig,
    pub html_path: &'a Path,
    pub svg_count: usize,
    pub extract_svg: bool,
}

impl<'a> HtmlContext<'a> {
    pub fn new(config: &'static SiteConfig, html_path: &'a Path) -> Self {
        Self {
            config,
            html_path,
            svg_count: 0,
            extract_svg: !matches!(
                config.build.typst.svg.extract_type,
                ExtractSvgType::Embedded
            ),
        }
    }
}

// ============================================================================
// SVG Extraction
// ============================================================================

/// Extract and optimize an SVG element, writing an img placeholder to the output
pub fn extract_svg_element(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: &BytesStart<'_>,
    ctx: &mut HtmlContext<'_>,
) -> Result<Option<Svg>> {
    // Process SVG data (transform, capture, optimize)
    let (optimized_data, size) = process_svg_data(reader, elem, ctx.config)?;

    // Create SVG and write placeholder
    let svg = Svg::new(optimized_data, size, ctx.svg_count);
    ctx.svg_count += 1;

    write_img_placeholder(writer, &svg, ctx)?;

    Ok(Some(svg))
}

/// Process SVG element: transform attributes, capture content, and optimize.
///
/// This function orchestrates the entire SVG processing pipeline:
/// 1. Transforms attributes (e.g., adjusting viewBox for padding).
/// 2. Captures the full SVG content from the reader.
/// 3. Optimizes the SVG using `usvg` and potentially compresses it.
fn process_svg_data(
    reader: &mut Reader<&[u8]>,
    elem: &BytesStart<'_>,
    config: &SiteConfig,
) -> Result<(Vec<u8>, (f32, f32))> {
    // Transform SVG attributes (adjust height/viewBox for typst quirks)
    let attrs = transform_svg_attrs(elem)?;

    // Capture complete SVG content
    let raw_svg = capture_svg_content(reader, &attrs)?;

    // Optimize with usvg
    optimize_svg(&raw_svg, config)
}

/// Transform SVG attributes (height, viewBox adjustments).
///
/// This function iterates through the attributes of the SVG element and applies
/// specific transformations required to fix layout quirks in Typst-generated SVGs.
/// Specifically, it adjusts `height` and `viewBox` to add padding.
fn transform_svg_attrs<'a>(elem: &'a BytesStart<'_>) -> Result<Vec<Attribute<'a>>> {
    let attrs = elem.attributes();
    // SVG typically has ~5-8 attributes
    let mut result = Vec::with_capacity(8);
    for attr in attrs.flatten() {
        let transformed = match attr.key.as_ref() {
            b"height" => adjust_height(attr)?,
            b"viewBox" => adjust_viewbox(attr)?,
            _ => attr,
        };
        result.push(transformed);
    }
    Ok(result)
}

/// Capture complete SVG element content from reader.
///
/// This function manually reconstructs the SVG content byte-by-byte to avoid
/// stability issues with `quick-xml`'s Writer when handling borrowed events.
/// It also optimizes performance by using `Vec<u8>` directly, avoiding unnecessary
/// UTF-8 validation overhead.
fn capture_svg_content(reader: &mut Reader<&[u8]>, attrs: &[Attribute<'_>]) -> Result<Vec<u8>> {
    let mut content = Vec::with_capacity(INITIAL_SVG_BUFFER_SIZE);

    // Write opening tag manually
    content.extend_from_slice(b"<svg");
    for attr in attrs {
        content.push(b' ');
        content.extend_from_slice(attr.key.as_ref());
        content.extend_from_slice(b"=\"");
        content.extend_from_slice(attr.value.as_ref());
        content.push(b'"');
    }
    content.push(b'>');

    // Capture nested content by tracking depth
    let mut depth = 1u32;
    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(e) => {
                depth += 1;
                content.push(b'<');
                content.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    content.push(b' ');
                    content.extend_from_slice(attr.key.as_ref());
                    content.extend_from_slice(b"=\"");
                    content.extend_from_slice(attr.value.as_ref());
                    content.push(b'"');
                }
                content.push(b'>');
            }
            Event::End(e) => {
                if e.name().as_ref() == b"svg" {
                    depth -= 1;
                    if depth == 0 {
                        content.extend_from_slice(b"</svg>");
                        break;
                    }
                } else {
                    depth -= 1;
                }
                content.extend_from_slice(b"</");
                content.extend_from_slice(e.name().as_ref());
                content.push(b'>');
            }
            Event::Empty(e) => {
                content.push(b'<');
                content.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    content.push(b' ');
                    content.extend_from_slice(attr.key.as_ref());
                    content.extend_from_slice(b"=\"");
                    content.extend_from_slice(attr.value.as_ref());
                    content.push(b'"');
                }
                content.extend_from_slice(b"/>");
            }
            Event::Text(e) => content.extend_from_slice(e.as_ref()),
            Event::CData(e) => {
                content.extend_from_slice(b"<![CDATA[");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"]]>");
            }
            Event::Comment(e) => {
                content.extend_from_slice(b"<!--");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"-->");
            }
            Event::Decl(e) => {
                content.extend_from_slice(b"<?");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"?>");
            }
            Event::PI(e) => {
                content.extend_from_slice(b"<?");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"?>");
            }
            Event::DocType(e) => {
                content.extend_from_slice(b"<!DOCTYPE ");
                content.extend_from_slice(e.as_ref());
                content.push(b'>');
            }
            Event::Eof => anyhow::bail!("Unexpected EOF while parsing SVG"),
            _ => {}
        }
    }

    Ok(content)
}

/// Optimize SVG using usvg, returning optimized bytes and dimensions.
///
/// This function parses the raw SVG data using `usvg`, which performs various
/// optimizations (cleaning up attributes, removing useless elements, etc.).
/// It then serializes the optimized tree back to a string and parses the dimensions.
fn optimize_svg(content: &[u8], config: &SiteConfig) -> Result<(Vec<u8>, (f32, f32))> {
    let options = usvg::Options {
        dpi: config.build.typst.svg.dpi,
        ..Default::default()
    };

    let tree = usvg::Tree::from_data(content, &options).context("Failed to parse SVG")?;

    let write_options = usvg::WriteOptions {
        indent: usvg::Indent::None,
        ..Default::default()
    };

    let optimized = tree.to_string(&write_options);
    let size = parse_dimensions(&optimized).unwrap_or((0.0, 0.0));

    Ok((optimized.into_bytes(), size))
}

/// Write img element as placeholder for extracted SVG
fn write_img_placeholder(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    svg: &Svg,
    ctx: &HtmlContext<'_>,
) -> Result<()> {
    let filename = svg.filename(ctx.config);
    let output_dir = ctx.html_path.parent().context("Invalid html path")?;
    let full_path = output_dir.join(&filename);

    // Build src attribute
    let src = url_from_output_path(&full_path, ctx.config).unwrap_or_else(|_| filename.clone());

    // Build style attribute with scaled dimensions
    let scale = ctx.config.get_scale();
    let (w, h) = (svg.size.0 / scale, svg.size.1 / scale);
    let mut style = String::with_capacity(40);
    let _ = write!(style, "width:{w}px;height:{h}px;");

    // Write img element
    let mut img = BytesStart::new("img");
    img.push_attribute(("src", src.as_str()));
    img.push_attribute(("style", style.as_str()));
    writer.write_event(Event::Start(img))?;

    Ok(())
}

// ============================================================================
// SVG Attribute Adjustments
// ============================================================================

/// Adjust height attribute (add top padding)
fn adjust_height(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let value = std::str::from_utf8(attr.value.as_ref())?;
    let height: f32 = value.trim_end_matches("pt").parse()?;

    Ok(Attribute {
        key: attr.key,
        value: format!("{}pt", height + SVG_PADDING_TOP)
            .into_bytes()
            .into(),
    })
}

/// Adjust viewBox attribute (expand for padding)
fn adjust_viewbox(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let value = std::str::from_utf8(attr.value.as_ref())?;

    // Parse 4 values without allocating a Vec
    let mut iter = value.split_whitespace();
    let (v0, v1, v2, v3) = match (iter.next(), iter.next(), iter.next(), iter.next()) {
        (Some(a), Some(b), Some(c), Some(d)) => (
            a.parse::<f32>().ok(),
            b.parse::<f32>().ok(),
            c.parse::<f32>().ok(),
            d.parse::<f32>().ok(),
        ),
        _ => anyhow::bail!("Invalid viewBox: expected 4 values"),
    };

    let (v0, v1, v2, v3) = match (v0, v1, v2, v3) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => anyhow::bail!("Invalid viewBox: failed to parse values"),
    };

    let new_viewbox = format!(
        "{} {} {} {}",
        v0,
        v1 - SVG_PADDING_TOP,
        v2,
        v3 + SVG_PADDING_TOP + SVG_PADDING_BOTTOM
    );

    Ok(Attribute {
        key: attr.key,
        value: new_viewbox.into_bytes().into(),
    })
}

// ============================================================================
// SVG Dimension Parsing
// ============================================================================

/// Parse width and height from SVG string (fast byte search)
fn parse_dimensions(svg: &str) -> Option<(f32, f32)> {
    let width = extract_attr(svg, r#"width=""#)?.parse().ok()?;
    let height = extract_attr(svg, r#"height=""#)?.parse().ok()?;
    Some((width, height))
}

/// Extract attribute value between prefix and closing quote
#[inline]
fn extract_attr<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    // Use bytes iterator for faster quote search
    let end = start + s.as_bytes()[start..].iter().position(|&b| b == b'"')?;
    Some(&s[start..end])
}

// ============================================================================
// SVG Compression
// ============================================================================

/// Compress multiple SVGs in parallel
pub fn compress_svgs_parallel(
    svgs: &[Svg],
    html_path: &Path,
    config: &'static SiteConfig,
) -> Result<()> {
    let output_dir = html_path.parent().context("Invalid html path")?;
    let log_prefix = get_log_prefix(html_path, config);
    let scale = config.get_scale();

    // Get HTML file's mtime for cache invalidation
    let html_mtime = html_path.metadata().and_then(|m| m.modified()).ok();

    svgs.par_iter().try_for_each(|svg| {
        let output_path = output_dir.join(svg.filename(config));

        if should_skip_compression(&output_path, html_mtime) {
            return Ok(());
        }

        log!("svg"; "{log_prefix}svg-{}", svg.index);
        compress_svg(svg, &output_path, scale, config)?;

        Ok(())
    })
}

fn get_log_prefix(html_path: &Path, config: &SiteConfig) -> String {
    let relative_path = html_path
        .strip_prefix(&config.build.output)
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    relative_path.trim_end_matches("index.html").to_string()
}

fn should_skip_compression(output_path: &Path, html_mtime: Option<SystemTime>) -> bool {
    if let Some(html_time) = html_mtime
        && let Ok(svg_time) = output_path.metadata().and_then(|m| m.modified())
        && svg_time >= html_time
    {
        return true;
    }
    false
}

/// Compress a single SVG based on configuration
fn compress_svg(svg: &Svg, output_path: &Path, scale: f32, config: &SiteConfig) -> Result<()> {
    // Small SVGs or JustSvg mode: write as-is
    if svg.output_format(config) == OutputFormat::Svg {
        return fs::write(output_path, &svg.data).map_err(Into::into);
    }

    // Compress to AVIF using configured backend
    match &config.build.typst.svg.extract_type {
        ExtractSvgType::Embedded | ExtractSvgType::JustSvg => {
            // Already handled above
            Ok(())
        }
        ExtractSvgType::Magick => compress_magick(output_path, &svg.data, scale),
        ExtractSvgType::Ffmpeg => compress_ffmpeg(output_path, &svg.data),
        ExtractSvgType::Builtin => compress_builtin(output_path, &svg.data, svg.size, scale),
    }
}

/// Compress using ImageMagick
fn compress_magick(output: &Path, data: &[u8], scale: f32) -> Result<()> {
    let density = (scale * 96.0).to_string();
    let mut proc = exec_with_stdin!(
        ["magick"];
        "-background", "none", "-density", density, "-", output
    )?;
    if let Some(stdin) = proc.stdin() {
        stdin.write_all(data)?;
    }
    proc.wait()
}

/// Compress using FFmpeg
fn compress_ffmpeg(output: &Path, data: &[u8]) -> Result<()> {
    let mut proc = exec_with_stdin!(
        ["ffmpeg"];
        "-f", "svg_pipe",
        "-frame_size", "1000000000",
        "-i", "pipe:",
        "-filter_complex", "[0:v]split[color][alpha];[alpha]alphaextract[alpha];[color]format=yuv420p[color]",
        "-map", "[color]",
        "-c:v:0", "libsvtav1",
        "-pix_fmt", "yuv420p",
        "-svtav1-params", "preset=4:still-picture=1",
        "-map", "[alpha]",
        "-c:v:1", "libaom-av1",
        "-pix_fmt", "gray",
        "-still-picture", "1",
        "-strict", "experimental",
        "-y", output
    )?;
    if let Some(stdin) = proc.stdin() {
        stdin.write_all(data)?;
    }
    proc.wait()
}

/// Compress using built-in ravif encoder
fn compress_builtin(output: &Path, data: &[u8], size: (f32, f32), scale: f32) -> Result<()> {
    let (width, height) = ((size.0 * scale) as usize, (size.1 * scale) as usize);
    let pixel_count = width * height;

    // Pre-allocate with exact capacity
    let mut pixmap = Vec::with_capacity(pixel_count);

    // Use chunks_exact for better optimization
    for c in data.chunks_exact(4) {
        pixmap.push(ravif::RGBA8::new(c[0], c[1], c[2], c[3]));
    }

    let encoded = ravif::Encoder::new()
        .with_quality(90.0)
        .with_speed(4)
        .encode_rgba(ravif::Img::new(&pixmap, width, height))?;

    fs::write(output, encoded.avif_file)?;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ------------------------------------------------------------------------
    // Dimension Parsing Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_parse_dimensions() {
        // Valid cases
        assert_eq!(
            parse_dimensions(r#"<svg width="100" height="50" xmlns="...">"#),
            Some((100.0, 50.0))
        );
        assert_eq!(
            parse_dimensions(r#"<svg width="123.5" height="67.8">"#),
            Some((123.5, 67.8))
        );
        // Reversed order should still work
        assert_eq!(
            parse_dimensions(r#"<svg height="200" width="300">"#),
            Some((300.0, 200.0))
        );

        // Missing attributes
        assert_eq!(parse_dimensions(r#"<svg height="50">"#), None);
        assert_eq!(parse_dimensions(r#"<svg width="100">"#), None);
        assert_eq!(parse_dimensions(r#"<svg>"#), None);

        // Invalid values
        assert_eq!(parse_dimensions(r#"<svg width="abc" height="50">"#), None);
    }

    #[test]
    fn test_extract_attr() {
        let s = r#"<svg width="100" height="50" class="icon">"#;
        assert_eq!(extract_attr(s, r#"width=""#), Some("100"));
        assert_eq!(extract_attr(s, r#"height=""#), Some("50"));
        assert_eq!(extract_attr(s, r#"class=""#), Some("icon"));
        assert_eq!(extract_attr(s, r#"id=""#), None);

        // Edge cases
        assert_eq!(extract_attr(r#"width="0""#, r#"width=""#), Some("0"));
        assert_eq!(extract_attr(r#"width="""#, r#"width=""#), Some(""));
    }

    // ------------------------------------------------------------------------
    // ViewBox Adjustment Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_adjust_viewbox_valid() {
        let attr = Attribute {
            key: quick_xml::name::QName(b"viewBox"),
            value: b"0 0 100 200".as_slice().into(),
        };
        let result = adjust_viewbox(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();

        // Check padding adjustments: y -= 5, height += 9
        assert!(value.contains("0 -5 100 209"));
    }

    #[test]
    fn test_adjust_viewbox_with_decimals() {
        let attr = Attribute {
            key: quick_xml::name::QName(b"viewBox"),
            value: b"0.5 1.5 100.5 200.5".as_slice().into(),
        };
        let result = adjust_viewbox(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert!(value.starts_with("0.5 -3.5")); // 1.5 - 5.0 = -3.5
    }

    #[test]
    fn test_adjust_viewbox_invalid() {
        // Not enough values
        let attr = Attribute {
            key: quick_xml::name::QName(b"viewBox"),
            value: b"0 0 100".as_slice().into(),
        };
        assert!(adjust_viewbox(attr).is_err());

        // Non-numeric values
        let attr = Attribute {
            key: quick_xml::name::QName(b"viewBox"),
            value: b"a b c d".as_slice().into(),
        };
        assert!(adjust_viewbox(attr).is_err());
    }

    // ------------------------------------------------------------------------
    // Height Adjustment Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_adjust_height() {
        let attr = Attribute {
            key: quick_xml::name::QName(b"height"),
            value: b"100pt".as_slice().into(),
        };
        let result = adjust_height(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert_eq!(value, "105pt"); // 100 + 5 padding
    }

    #[test]
    fn test_adjust_height_decimal() {
        let attr = Attribute {
            key: quick_xml::name::QName(b"height"),
            value: b"50.5pt".as_slice().into(),
        };
        let result = adjust_height(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert_eq!(value, "55.5pt");
    }

    // ------------------------------------------------------------------------
    // Path Building Tests
    // ------------------------------------------------------------------------

    // test_build_src_path removed as it tests removed function.
    // Logic moved to utils::page::url_from_output_path


    // ------------------------------------------------------------------------
    // Output Format Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_output_format() {
        let config = Box::leak(Box::new(SiteConfig::default()));
        let inline_max = config.get_inline_max_size();

        // Small SVG -> Svg format
        let small = Svg::new(vec![0; inline_max - 1], (10.0, 10.0), 0);
        assert_eq!(small.output_format(config), OutputFormat::Svg);

        // Large SVG -> Avif format (when not JustSvg mode)
        let large = Svg::new(vec![0; inline_max + 1], (100.0, 100.0), 1);
        assert_eq!(large.output_format(config), OutputFormat::Avif);
    }

    #[test]
    fn test_svg_filename() {
        let config = Box::leak(Box::new(SiteConfig::default()));

        // Small SVG gets .svg extension
        let small = Svg::new(vec![0; 10], (10.0, 10.0), 5);
        assert_eq!(small.filename(config), "svg-5.svg");

        // Large SVG gets .avif extension
        let large = Svg::new(vec![0; 100_000], (100.0, 100.0), 3);
        assert_eq!(large.filename(config), "svg-3.avif");
    }

    // ------------------------------------------------------------------------
    // SVG Struct Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_svg_new() {
        let data = vec![1, 2, 3, 4];
        let svg = Svg::new(data.clone(), (100.5, 200.5), 42);

        assert_eq!(svg.data, data);
        assert_eq!(svg.size, (100.5, 200.5));
        assert_eq!(svg.index, 42);
    }

    // ------------------------------------------------------------------------
    // HtmlContext Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_html_context_extract_svg_flag() {
        let mut config = SiteConfig::default();

        // Embedded mode: don't extract
        config.build.typst.svg.extract_type = ExtractSvgType::Embedded;
        let config = Box::leak(Box::new(config));
        let ctx = HtmlContext::new(config, Path::new("/test.html"));
        assert!(!ctx.extract_svg);
    }

    // ------------------------------------------------------------------------
    // Logic Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_get_log_prefix() {
        let mut config = SiteConfig::default();
        config.build.output = PathBuf::from("public");

        let path = PathBuf::from("public/blog/post/index.html");
        assert_eq!(get_log_prefix(&path, &config), "blog/post/");

        let path = PathBuf::from("public/index.html");
        assert_eq!(get_log_prefix(&path, &config), "");
    }

    #[test]
    fn test_capture_svg_content() {
        // Input XML fragment (simulating content after <svg> tag)
        // Note: capture_svg_content expects to read until </svg>
        // We must provide a full SVG so the reader tracks the opening tag
        let xml = r#"<svg><rect x="0" y="0" width="10" height="10" /></svg>"#;
        let mut reader = Reader::from_str(xml);
        // reader.config_mut().trim_text(true);
        reader.config_mut().check_end_names = false;

        // Advance past the opening <svg> tag to simulate the state when capture_svg_content is called
        let _ = reader.read_event().unwrap();

        let attrs = vec![];
        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        // Should wrap in <svg>...</svg>
        assert_eq!(s, r#"<svg><rect x="0" y="0" width="10" height="10"/></svg>"#);
    }

    #[test]
    fn test_capture_svg_content_nested() {
        let xml = r#"<svg><g><svg viewBox="0 0 10 10"><rect /></svg></g></svg>"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = false;

        // Advance past the opening <svg> tag
        let _ = reader.read_event().unwrap();

        let attrs = vec![];
        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        assert_eq!(s, r#"<svg><g><svg viewBox="0 0 10 10"><rect/></svg></g></svg>"#);
    }

    #[test]
    fn test_capture_svg_content_with_attrs() {
        let xml = r#"<svg><rect /></svg>"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = false;
        let _ = reader.read_event().unwrap();

        let attrs = vec![
            Attribute { key: quick_xml::name::QName(b"width"), value: b"100".as_slice().into() },
            Attribute { key: quick_xml::name::QName(b"height"), value: b"100".as_slice().into() },
        ];

        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        assert_eq!(s, r#"<svg width="100" height="100"><rect/></svg>"#);
    }

    #[test]
    fn test_transform_svg_attrs() {
        let xml = r#"<svg width="100" height="100pt" viewBox="0 0 100 100">"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        if let Ok(Event::Start(ref e)) = reader.read_event() {
            let attrs = transform_svg_attrs(e).unwrap();

            // Check if attributes are transformed
            let mut found_height = false;
            let mut found_viewbox = false;

            for attr in attrs {
                match attr.key.as_ref() {
                    b"height" => {
                        found_height = true;
                        assert_eq!(attr.value.as_ref(), b"105pt"); // 100 + 5
                    }
                    b"viewBox" => {
                        found_viewbox = true;
                        // 0 0 100 100 -> 0 -5 100 109
                        assert_eq!(attr.value.as_ref(), b"0 -5 100 109");
                    }
                    b"width" => {
                        assert_eq!(attr.value.as_ref(), b"100");
                    }
                    _ => {}
                }
            }
            assert!(found_height);
            assert!(found_viewbox);
        } else {
            panic!("Failed to read start event");
        }
    }
}
