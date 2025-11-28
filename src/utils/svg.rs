//! SVG extraction and processing utilities.
//!
//! Handles SVG extraction from HTML, optimization, and compression to various formats.

use anyhow::{Context, Result};
use quick_xml::{
    Reader, Writer,
    events::{BytesStart, Event, attributes::Attribute},
};
use rayon::prelude::*;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::str;

use crate::config::{ExtractSvgType, SiteConfig};
use crate::{log, run_command_with_stdin};

// ============================================================================
// Constants
// ============================================================================

const PADDING_TOP_FOR_SVG: f32 = 5.0;
const PADDING_BOTTOM_FOR_SVG: f32 = 4.0;

// ============================================================================
// Types
// ============================================================================

/// Extracted SVG data with dimensions
pub struct Svg {
    pub data: Vec<u8>,
    pub size: (f32, f32),
    pub index: usize,
}

impl Svg {
    pub fn new(data: Vec<u8>, size: (f32, f32), index: usize) -> Self {
        Self { data, size, index }
    }

    /// Determine output filename based on extract type and size
    pub fn output_filename(&self, config: &SiteConfig) -> String {
        let use_svg = matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || self.data.len() < config.get_inline_max_size();

        if use_svg {
            format!("svg-{}.svg", self.index)
        } else {
            format!("svg-{}.avif", self.index)
        }
    }

    /// Check if this SVG should be kept as SVG (not compressed to AVIF)
    pub fn should_keep_as_svg(&self, config: &SiteConfig) -> bool {
        matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || self.data.len() < config.get_inline_max_size()
    }
}

/// Context for HTML processing, avoiding repeated config access
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

/// Extract SVG element from XML reader, returning optimized SVG data
pub fn extract_svg_element(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: &BytesStart<'_>,
    ctx: &mut HtmlContext<'_>,
) -> Result<Option<Svg>> {
    // Filter and transform SVG attributes
    let attrs: Vec<_> = elem
        .attributes()
        .flatten()
        .filter_map(|attr| match attr.key.as_ref() {
            b"height" => adjust_height_attr(attr).ok(),
            b"viewBox" => adjust_viewbox_attr(attr).ok(),
            _ => Some(attr),
        })
        .collect();

    // Capture SVG content
    let svg_content = capture_svg_content(reader, &attrs)?;

    // Parse and optimize SVG
    let (svg_data, size) = optimize_svg(&svg_content, ctx.config)?;

    // Write img placeholder to HTML
    let svg_index = ctx.svg_count;
    ctx.svg_count += 1;

    let svg = Svg::new(svg_data, size, svg_index);
    write_svg_img_placeholder(writer, &svg, ctx)?;

    Ok(Some(svg))
}

/// Capture SVG content from reader into a byte buffer
pub fn capture_svg_content(reader: &mut Reader<&[u8]>, attrs: &[Attribute<'_>]) -> Result<Vec<u8>> {
    let mut svg_writer = Writer::new(Cursor::new(Vec::with_capacity(4096)));
    svg_writer.write_event(Event::Start(
        BytesStart::new("svg").with_attributes(attrs.iter().cloned()),
    ))?;

    let mut depth = 1u32;
    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(e) if e.name().as_ref() == b"svg" => {
                depth -= 1;
                if depth == 0 {
                    svg_writer.write_event(event)?;
                    break;
                }
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
        svg_writer.write_event(event)?;
    }

    Ok(svg_writer.into_inner().into_inner())
}

/// Optimize SVG content using usvg
pub fn optimize_svg(svg_content: &[u8], config: &SiteConfig) -> Result<(Vec<u8>, (f32, f32))> {
    let opt = usvg::Options {
        dpi: config.build.typst.svg.dpi,
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(svg_content, &opt).context("Failed to parse SVG")?;

    let write_opt = usvg::WriteOptions {
        indent: usvg::Indent::None,
        ..Default::default()
    };
    let optimized = tree.to_string(&write_opt);
    let size = parse_svg_dimensions(&optimized).unwrap_or((0.0, 0.0));

    Ok((optimized.into_bytes(), size))
}

/// Write an img element as placeholder for extracted SVG
pub fn write_svg_img_placeholder(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    svg: &Svg,
    ctx: &HtmlContext<'_>,
) -> Result<()> {
    let svg_filename = svg.output_filename(ctx.config);
    let svg_path = ctx.html_path.parent().unwrap().join(&svg_filename);
    let src = svg_path
        .strip_prefix(&ctx.config.build.output)
        .map(|p| format!("/{}", p.display()))
        .unwrap_or_else(|_| svg_filename);

    let scale = ctx.config.get_scale();
    let (w, h) = svg.size;
    let style = format!("width:{}px;height:{}px;", w / scale, h / scale);

    let mut img = BytesStart::new("img");
    img.push_attribute(("src", src.as_str()));
    img.push_attribute(("style", style.as_str()));
    writer.write_event(Event::Start(img))?;

    Ok(())
}

// ============================================================================
// SVG Attribute Adjustments
// ============================================================================

/// Adjust height attribute for SVG element
pub fn adjust_height_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let height_str = str::from_utf8(attr.value.as_ref())?;
    let height: f32 = height_str.trim_end_matches("pt").parse()?;
    let new_height = height + PADDING_TOP_FOR_SVG;

    Ok(Attribute {
        key: attr.key,
        value: format!("{new_height}pt").into_bytes().into(),
    })
}

/// Adjust viewBox attribute for SVG element
pub fn adjust_viewbox_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let viewbox_str = str::from_utf8(attr.value.as_ref())?;
    let parts: Vec<f32> = viewbox_str
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();

    if parts.len() != 4 {
        anyhow::bail!("Invalid viewBox format");
    }

    let new_viewbox = format!(
        "{} {} {} {}",
        parts[0],
        parts[1] - PADDING_TOP_FOR_SVG,
        parts[2],
        parts[3] + PADDING_BOTTOM_FOR_SVG + PADDING_TOP_FOR_SVG
    );

    Ok(Attribute {
        key: attr.key,
        value: new_viewbox.into_bytes().into(),
    })
}

// ============================================================================
// SVG Dimension Parsing
// ============================================================================

/// Parse width and height from SVG string (fast string search, no regex)
pub fn parse_svg_dimensions(svg_data: &str) -> Option<(f32, f32)> {
    let width = extract_attr_value(svg_data, "width=\"")?.parse().ok()?;
    let height = extract_attr_value(svg_data, "height=\"")?.parse().ok()?;
    Some((width, height))
}

/// Extract attribute value from string
#[inline]
pub fn extract_attr_value<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    let end = s[start..].find('"')? + start;
    Some(&s[start..end])
}

// ============================================================================
// SVG Compression (Parallel)
// ============================================================================

/// Compress multiple SVGs in parallel
pub fn compress_svgs_parallel(
    svgs: &[Svg],
    html_path: &Path,
    config: &'static SiteConfig,
) -> Result<()> {
    let parent = html_path.parent().context("Invalid html path")?;
    let relative_path = html_path
        .strip_prefix(&config.build.output)
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let relative_path = relative_path.trim_end_matches("index.html");
    let scale = config.get_scale();

    svgs.par_iter().try_for_each(|svg| {
        log!("svg"; "in {relative_path}: compress svg-{}", svg.index);

        let svg_path = parent.join(svg.output_filename(config));
        compress_single_svg(svg, &svg_path, scale, config)?;

        log!("svg"; "in {relative_path}: finish compressing svg-{}", svg.index);
        Ok(())
    })
}

/// Compress a single SVG to the appropriate output format
pub fn compress_single_svg(
    svg: &Svg,
    output_path: &Path,
    scale: f32,
    config: &SiteConfig,
) -> Result<()> {
    if svg.should_keep_as_svg(config) {
        return fs::write(output_path, &svg.data).map_err(Into::into);
    }

    match &config.build.typst.svg.extract_type {
        ExtractSvgType::Embedded => Ok(()),
        ExtractSvgType::JustSvg => fs::write(output_path, &svg.data).map_err(Into::into),
        ExtractSvgType::Magick => compress_with_magick(output_path, &svg.data, scale),
        ExtractSvgType::Ffmpeg => compress_with_ffmpeg(output_path, &svg.data),
        ExtractSvgType::Builtin => compress_with_builtin(output_path, &svg.data, svg.size, scale),
    }
}

/// Compress SVG using ImageMagick
pub fn compress_with_magick(output_path: &Path, svg_data: &[u8], scale: f32) -> Result<()> {
    let density = (scale * 96.0).to_string();
    let mut stdin = run_command_with_stdin!(
        ["magick"];
        "-background", "none", "-density", density, "-", output_path
    )?;
    stdin.write_all(svg_data)?;
    Ok(())
}

/// Compress SVG using FFmpeg
pub fn compress_with_ffmpeg(output_path: &Path, svg_data: &[u8]) -> Result<()> {
    let mut stdin = run_command_with_stdin!(
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
        "-c:v", "libaom-av1",
        "-y", output_path
    )?;
    stdin.write_all(svg_data)?;
    Ok(())
}

/// Compress SVG using built-in ravif encoder
pub fn compress_with_builtin(
    output_path: &Path,
    svg_data: &[u8],
    size: (f32, f32),
    scale: f32,
) -> Result<()> {
    let (width, height) = ((size.0 * scale) as usize, (size.1 * scale) as usize);

    let pixmap: Vec<_> = svg_data
        .chunks(4)
        .map(|chunk| ravif::RGBA8::new(chunk[0], chunk[1], chunk[2], chunk[3]))
        .collect();

    let img = ravif::Encoder::new()
        .with_quality(90.0)
        .with_speed(4)
        .encode_rgba(ravif::Img::new(&pixmap, width, height))?;

    fs::write(output_path, img.avif_file)?;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_svg_dimensions_valid() {
        let svg = r#"<svg width="100" height="50" xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let dims = parse_svg_dimensions(svg);
        assert_eq!(dims, Some((100.0, 50.0)));
    }

    #[test]
    fn test_parse_svg_dimensions_with_decimals() {
        let svg = r#"<svg width="123.5" height="67.8" xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let dims = parse_svg_dimensions(svg);
        assert_eq!(dims, Some((123.5, 67.8)));
    }

    #[test]
    fn test_parse_svg_dimensions_missing_width() {
        let svg = r#"<svg height="50" xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let dims = parse_svg_dimensions(svg);
        assert_eq!(dims, None);
    }

    #[test]
    fn test_parse_svg_dimensions_missing_height() {
        let svg = r#"<svg width="100" xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let dims = parse_svg_dimensions(svg);
        assert_eq!(dims, None);
    }

    #[test]
    fn test_extract_attr_value_found() {
        let s = r#"<svg width="100" height="50">"#;
        assert_eq!(extract_attr_value(s, "width=\""), Some("100"));
        assert_eq!(extract_attr_value(s, "height=\""), Some("50"));
    }

    #[test]
    fn test_extract_attr_value_not_found() {
        let s = r#"<svg width="100">"#;
        assert_eq!(extract_attr_value(s, "height=\""), None);
    }

    #[test]
    fn test_svg_output_filename() {
        let config = Box::leak(Box::new(SiteConfig::default()));
        let svg = Svg::new(vec![0; 100], (100.0, 50.0), 3);
        let filename = svg.output_filename(config);
        // Default config should use JustSvg or small size threshold
        assert!(filename.ends_with(".svg") || filename.ends_with(".avif"));
        assert!(filename.contains("svg-3"));
    }

    #[test]
    fn test_svg_should_keep_as_svg_small_size() {
        let config = Box::leak(Box::new(SiteConfig::default()));
        // Create a very small SVG that should stay as SVG
        let svg = Svg::new(vec![0; 10], (10.0, 10.0), 0);
        assert!(svg.should_keep_as_svg(config));
    }
}
