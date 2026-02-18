//! SVG optimization using usvg.
//!
//! Handles SVG minification and viewBox expansion to include stroke boundaries.

use anyhow::{Context, Result};

use super::bounds::{calculate_stroke_bounds, expand_viewbox_to_bounds};

/// Options for SVG optimization
#[derive(Debug, Clone)]
pub struct OptimizeOptions {
    /// DPI for rendering calculations.
    pub dpi: f32,
    /// Whether to expand viewBox to include stroke boundaries.
    /// This prevents content clipping when converting to external files.
    pub expand_viewbox: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            dpi: 96.0,
            expand_viewbox: true,
        }
    }
}

/// Optimized SVG result
pub struct OptimizedSvg {
    /// Optimized SVG content as bytes.
    pub data: Vec<u8>,
    /// Dimensions (width, height) in pixels.
    pub size: (f32, f32),
}

/// Optimize SVG using usvg
///
/// Returns optimized SVG bytes and dimensions
///
/// When `expand_viewbox` is enabled, calculates the stroke-inclusive bounding box
/// and expands the viewBox to prevent content clipping
pub fn optimize_svg(content: &[u8], options: &OptimizeOptions) -> Result<OptimizedSvg> {
    let usvg_options = usvg::Options {
        dpi: options.dpi,
        ..Default::default()
    };

    let tree = usvg::Tree::from_data(content, &usvg_options).context("Failed to parse SVG")?;

    let write_options = usvg::WriteOptions {
        indent: usvg::Indent::None,
        ..Default::default()
    };

    let mut optimized = tree.to_string(&write_options);

    // Expand viewBox to include stroke boundaries if enabled
    if options.expand_viewbox
        && let Some(bounds) = calculate_stroke_bounds(&tree)
    {
        optimized = expand_viewbox_to_bounds(&optimized, bounds);
    }

    let size = parse_dimensions(&optimized).unwrap_or((0.0, 0.0));

    Ok(OptimizedSvg {
        data: optimized.into_bytes(),
        size,
    })
}

/// Parse width and height from SVG string
fn parse_dimensions(svg: &str) -> Option<(f32, f32)> {
    let width = extract_attr(svg, r#"width=""#)?.parse().ok()?;
    let height = extract_attr(svg, r#"height=""#)?.parse().ok()?;
    Some((width, height))
}

/// Extract attribute value between prefix and closing quote
#[inline]
fn extract_attr<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    let end = start + s.as_bytes()[start..].iter().position(|&b| b == b'"')?;
    Some(&s[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dimensions() {
        assert_eq!(
            parse_dimensions(r#"<svg width="100" height="50" xmlns="...">"#),
            Some((100.0, 50.0))
        );
        assert_eq!(
            parse_dimensions(r#"<svg width="123.5" height="67.8">"#),
            Some((123.5, 67.8))
        );
        assert_eq!(parse_dimensions(r#"<svg height="50">"#), None);
    }

    #[test]
    fn test_extract_attr() {
        let s = r#"<svg width="100" height="50" class="icon">"#;
        assert_eq!(extract_attr(s, r#"width=""#), Some("100"));
        assert_eq!(extract_attr(s, r#"height=""#), Some("50"));
        assert_eq!(extract_attr(s, r#"id=""#), None);
    }
}
