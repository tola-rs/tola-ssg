//! SVG file extraction.
//!
//! Handles extracting SVG to external files with content-hash based naming.

#![allow(dead_code)]

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use super::convert::convert_svg;
use super::filename_hash;
use super::optimize::{OptimizeOptions, optimize_svg};
use crate::config::{SvgConverter, SvgFormat};

/// Context for SVG extraction.
#[derive(Debug, Clone)]
pub struct ExtractContext {
    /// Output directory for the page (e.g., `public/posts/hello/`).
    pub output_dir: PathBuf,
    /// Target format for conversion.
    pub format: SvgFormat,
    /// Conversion backend.
    pub converter: SvgConverter,
    /// DPI for rendering.
    pub dpi: f32,
    /// Quality for lossy formats (0-100).
    pub quality: u8,
    /// Size threshold in bytes - SVGs smaller than this stay inline.
    pub threshold: usize,
    /// Whether to expand viewBox to include stroke boundaries.
    pub expand_viewbox: bool,
}

impl ExtractContext {
    /// Create a new extract context from config.
    pub fn new(
        output_dir: PathBuf,
        format: SvgFormat,
        converter: SvgConverter,
        dpi: f32,
        threshold: usize,
        expand_viewbox: bool,
    ) -> Self {
        Self {
            output_dir,
            format,
            converter,
            dpi,
            quality: 90,
            threshold,
            expand_viewbox,
        }
    }

    /// Get the .tola subdirectory path.
    pub fn tola_dir(&self) -> PathBuf {
        self.output_dir.join(".tola")
    }

    /// Check if SVG should stay inline based on size threshold.
    pub fn should_inline(&self, svg_size: usize) -> bool {
        self.threshold > 0 && svg_size < self.threshold
    }
}

/// Result of SVG extraction.
pub struct ExtractResult {
    /// Relative path to the extracted file (e.g., `.tola/svg-a1b2c3d4e5f6.avif`).
    pub relative_path: String,
    /// Absolute path to the extracted file.
    pub absolute_path: PathBuf,
    /// Whether the file was newly created (false if already existed).
    pub created: bool,
}

/// Extract SVG to an external file.
///
/// # Process
/// 1. Optimize SVG using usvg
/// 2. Convert to target format (AVIF/PNG/etc.) if needed
/// 3. Generate content-hash filename
/// 4. Write to `.tola/` subdirectory (skip if exists)
/// 5. Return relative path for HTML replacement
///
/// # Arguments
/// * `svg_content` - Raw SVG content bytes
/// * `ctx` - Extraction context with output settings
///
/// # Returns
/// `ExtractResult` with the relative path to use in HTML.
pub fn extract_svg_to_file(svg_content: &[u8], ctx: &ExtractContext) -> Result<ExtractResult> {
    let optimize_opts = OptimizeOptions {
        dpi: ctx.dpi,
        expand_viewbox: ctx.expand_viewbox,
    };
    let optimized = optimize_svg(svg_content, &optimize_opts).context("Failed to optimize SVG")?;

    let converted = convert_svg(
        &optimized.data,
        optimized.size,
        &ctx.format,
        &ctx.converter,
        ctx.dpi,
        ctx.quality,
    )
    .context("Failed to convert SVG")?;

    let hash = filename_hash(&converted);
    let filename = format!("svg-{}.{}", hash, ctx.format.extension());

    let tola_dir = ctx.tola_dir();
    let output_path = tola_dir.join(&filename);

    // Skip if exists (incremental build optimization)
    let created = if !output_path.exists() {
        fs::create_dir_all(&tola_dir)
            .with_context(|| format!("Failed to create {}", tola_dir.display()))?;
        fs::write(&output_path, &converted)
            .with_context(|| format!("Failed to write {}", output_path.display()))?;
        true
    } else {
        false
    };

    let relative_path = format!(".tola/{}", filename);

    Ok(ExtractResult {
        relative_path,
        absolute_path: output_path,
        created,
    })
}

/// Check if an extracted file already exists for the given SVG content.
///
/// Useful for incremental builds to skip re-extraction.
pub fn check_extracted_exists(svg_content: &[u8], ctx: &ExtractContext) -> Option<PathBuf> {
    // Quick hash check without full optimization
    let hash = filename_hash(svg_content);
    let filename = format!("svg-{}.{}", hash, ctx.format.extension());
    let path = ctx.tola_dir().join(&filename);

    if path.exists() { Some(path) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_context_tola_dir() {
        let ctx = ExtractContext::new(
            PathBuf::from("/public/posts/hello"),
            SvgFormat::AVIF,
            SvgConverter::Builtin,
            96.0,
            0,
            true,
        );
        assert_eq!(ctx.tola_dir(), PathBuf::from("/public/posts/hello/.tola"));
    }

    #[test]
    fn test_should_inline() {
        let ctx = ExtractContext::new(
            PathBuf::from("/public"),
            SvgFormat::AVIF,
            SvgConverter::Builtin,
            96.0,
            10 * 1024, // 10KB threshold
            true,
        );

        assert!(ctx.should_inline(5 * 1024)); // 5KB < 10KB
        assert!(!ctx.should_inline(15 * 1024)); // 15KB > 10KB

        // No threshold = never inline
        let ctx_no_threshold = ExtractContext::new(
            PathBuf::from("/public"),
            SvgFormat::AVIF,
            SvgConverter::Builtin,
            96.0,
            0,
            true,
        );
        assert!(!ctx_no_threshold.should_inline(100));
    }
}
