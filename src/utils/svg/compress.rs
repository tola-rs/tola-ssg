use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use crate::config::{ExtractSvgType, SiteConfig};
use crate::{exec_with_stdin, log};
use super::{Svg, OutputFormat};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_log_prefix() {
        let mut config = SiteConfig::default();
        config.build.output = PathBuf::from("public");

        let path = PathBuf::from("public/blog/post/index.html");
        assert_eq!(get_log_prefix(&path, &config), "blog/post/");

        let path = PathBuf::from("public/index.html");
        assert_eq!(get_log_prefix(&path, &config), "");
    }
}
