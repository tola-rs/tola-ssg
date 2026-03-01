//! SVG format conversion.
//!
//! Converts SVG to raster formats (PNG, WebP, JPG) using various backends.

use anyhow::{Context, Result};

use crate::config::{SvgConverter, SvgFormat};
use crate::utils::exec::Cmd;

/// Convert SVG to the specified format
///
/// # Arguments
/// * `svg_data` - Optimized SVG bytes
/// * `size` - SVG dimensions (width, height) in pixels
/// * `format` - Target output format
/// * `converter` - Conversion backend to use
/// * `dpi` - DPI for rendering (affects output resolution)
/// * `quality` - Quality for lossy formats (0-100)
///
/// # Returns
/// Converted image bytes, or error if conversion fails
///
/// # Note
/// If `format` is `SVG`, returns the input unchanged (no conversion needed)
pub fn convert_svg(
    svg_data: &[u8],
    size: (f32, f32),
    format: &SvgFormat,
    converter: &SvgConverter,
    dpi: f32,
    quality: u8,
) -> Result<Vec<u8>> {
    // SVG format = no conversion needed
    if matches!(format, SvgFormat::SVG) {
        return Ok(svg_data.to_vec());
    }

    match converter {
        SvgConverter::Builtin => convert_builtin(svg_data, size, format, dpi, quality),
        SvgConverter::Magick => convert_magick(svg_data, format, dpi),
        SvgConverter::Ffmpeg => convert_ffmpeg(svg_data, format),
    }
}

/// Convert using built-in Rust libraries
///
/// Requires `resvg` for SVG rendering and format-specific encoders
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn convert_builtin(
    svg_data: &[u8],
    size: (f32, f32),
    format: &SvgFormat,
    dpi: f32,
    quality: u8,
) -> Result<Vec<u8>> {
    // Parse SVG using usvg
    let tree = usvg::Tree::from_data(svg_data, &usvg::Options::default())
        .context("Failed to parse SVG")?;

    // Calculate output dimensions
    let scale = dpi / 96.0;
    let width = (size.0 * scale) as u32;
    let height = (size.1 * scale) as u32;

    if width == 0 || height == 0 {
        anyhow::bail!("Invalid SVG dimensions: {}x{}", size.0, size.1);
    }

    // Render SVG to pixels using tiny-skia
    // Note: This requires the `resvg` crate with `tiny-skia` feature
    // For now, we'll use a simpler approach or fall back to external tools

    // TODO: Add resvg dependency for proper rendering
    // For now, return error suggesting to use magick/ffmpeg
    match format {
        SvgFormat::PNG => render_and_encode_png(&tree, width, height, scale),
        _ => {
            let _ = quality;
            anyhow::bail!(
                "Builtin converter does not support {} format yet. Use magick or ffmpeg.",
                format.extension()
            )
        }
    }
}

/// Render SVG tree and encode to PNG
fn render_and_encode_png(
    tree: &usvg::Tree,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Vec<u8>> {
    let pixels = render_svg_to_rgba(tree, width, height, scale)?;
    // PNG encoding requires the `png` crate
    let _ = pixels;
    anyhow::bail!(
        "Builtin PNG encoding requires the `png` crate. \
         Please use `converter = \"magick\"` or `converter = \"ffmpeg\"` instead."
    )
}

/// Render SVG to RGBA pixels
///
/// Note: This is a placeholder. Full implementation requires `resvg` crate
fn render_svg_to_rgba(
    _tree: &usvg::Tree,
    _width: u32,
    _height: u32,
    _scale: f32,
) -> Result<Vec<u8>> {
    // TODO: Implement with resvg
    // let mut pixmap = tiny_skia::Pixmap::new(width, height).context("Failed to create pixmap")?;
    // resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // Ok(pixmap.data().to_vec())

    anyhow::bail!(
        "Builtin SVG rendering requires the `resvg` crate. \
         Please use `converter = \"magick\"` or `converter = \"ffmpeg\"` instead."
    )
}

/// Convert using ImageMagick
fn convert_magick(svg_data: &[u8], format: &SvgFormat, dpi: f32) -> Result<Vec<u8>> {
    let density = dpi.to_string();
    let format_arg = format!("{}:-", format.extension());

    let output = Cmd::new("magick")
        .args([
            "-background",
            "none",
            "-density",
            &density,
            "-",
            &format_arg,
        ])
        .stdin(svg_data)
        .run()
        .context("ImageMagick conversion failed")?;

    Ok(output.stdout)
}

/// Convert using FFmpeg
fn convert_ffmpeg(svg_data: &[u8], format: &SvgFormat) -> Result<Vec<u8>> {
    let format_args: &[&str] = match format {
        SvgFormat::PNG => &["-f", "image2pipe", "-c:v", "png"],
        SvgFormat::WEBP => &["-c:v", "libwebp", "-f", "webp"],
        SvgFormat::JPG => &["-c:v", "mjpeg", "-f", "image2pipe"],
        SvgFormat::SVG => return Ok(svg_data.to_vec()), // Should not reach here
    };

    let output = Cmd::new("ffmpeg")
        .args(["-f", "svg_pipe", "-frame_size", "1000000000", "-i", "pipe:"])
        .args(format_args)
        .arg("pipe:1")
        .stdin(svg_data)
        .run()
        .context("FFmpeg conversion failed")?;

    Ok(output.stdout)
}
