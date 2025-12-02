mod compress;
mod extract;
mod optimize;
mod transform;

pub use compress::compress_svgs_parallel;
pub use extract::extract_svg_element;

use crate::config::{ExtractSvgType, SiteConfig};
use std::path::Path;

// ============================================================================
// Constants
// ============================================================================

/// Padding adjustments for SVG viewBox (typst HTML output quirks)
pub const SVG_PADDING_TOP: f32 = 5.0;
pub const SVG_PADDING_BOTTOM: f32 = 4.0;

/// Initial buffer size for capturing SVG content (16KB)
pub const INITIAL_SVG_BUFFER_SIZE: usize = 16 * 1024;

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
pub enum OutputFormat {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    #[test]
    fn test_svg_new() {
        let data = vec![1, 2, 3, 4];
        let svg = Svg::new(data.clone(), (100.5, 200.5), 42);

        assert_eq!(svg.data, data);
        assert_eq!(svg.size, (100.5, 200.5));
        assert_eq!(svg.index, 42);
    }

    #[test]
    fn test_html_context_extract_svg_flag() {
        let mut config = SiteConfig::default();

        // Embedded mode: don't extract
        config.build.typst.svg.extract_type = ExtractSvgType::Embedded;
        let config = Box::leak(Box::new(config));
        let ctx = HtmlContext::new(config, Path::new("/test.html"));
        assert!(!ctx.extract_svg);
    }
}
