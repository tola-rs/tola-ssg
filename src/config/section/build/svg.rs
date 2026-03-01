//! `[build.svg]` section configuration. (WIP)
//!
//! SVG processing settings for extraction and conversion.
//!
//! # Example
//!
//! ```toml
//! [build.svg]
//! external = true         # true = separate files, false = embedded in HTML
//! format = "svg"          # Output format: svg | png | jpg | webp
//! converter = "builtin"   # Conversion backend: builtin | magick | ffmpeg
//! dpi = 144.0             # Rendering DPI (default: 96.0)
//! threshold = "10KB"      # SVGs smaller than this stay inline
//! expand_viewbox = true   # Auto-expand viewBox to include stroke (default: true)
//! baseline_align = false  # Apply vertical-align for inline SVG baseline (default: false)
//! ```
//!
//! # Behavior
//!
//! - `external = false` -> SVG embedded in HTML (other options ignored)
//! - `external = true, format = "svg"` -> Extract as SVG file (no conversion)
//! - `external = true, format = "png|jpg|webp"` -> Convert to raster image using `converter`

use macros::Config;
use serde::{Deserialize, Serialize};

/// SVG output format
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SvgFormat {
    /// Keep as SVG (no rasterization).
    #[default]
    SVG,
    /// PNG format.
    PNG,
    /// JPEG format.
    JPG,
    /// WebP format.
    WEBP,
}

impl SvgFormat {
    /// Get file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::SVG => "svg",
            Self::PNG => "png",
            Self::JPG => "jpg",
            Self::WEBP => "webp",
        }
    }

    /// Check if this format requires rasterization.
    pub fn needs_rasterization(&self) -> bool {
        !matches!(self, Self::SVG)
    }
}

/// SVG conversion backend
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SvgConverter {
    /// Use built-in Rust libraries.
    #[default]
    Builtin,
    /// Use ImageMagick (`magick` command).
    Magick,
    /// Use FFmpeg for conversion.
    Ffmpeg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.svg", status = experimental)]
pub struct SvgConfig {
    /// Extract SVG to separate files (true) or embed in HTML (false).
    pub external: bool,

    /// Output format for extracted SVGs.
    /// - `svg`: Keep as optimized SVG (no rasterization)
    /// - `png`/`jpg`/`webp`: Convert to raster image
    #[config(default = "svg")]
    pub format: SvgFormat,

    /// Conversion backend for rasterization.
    /// Only used when `format` is not `svg`.
    #[config(default = "builtin")]
    pub converter: SvgConverter,

    /// DPI for SVG rendering.
    #[config(default = "96.0")]
    pub dpi: f32,

    /// Size threshold - SVGs smaller than this stay inline even if `external = true`.
    /// Supports suffixes: B, KB, MB (e.g., "10KB", "1MB").
    #[config(default = "0B")]
    pub threshold: String,

    /// Auto-expand viewBox to include stroke boundaries.
    /// Prevents content clipping when converting to external files.
    /// Default: true
    #[config(default = "true")]
    pub expand_viewbox: bool,

    /// Apply vertical-align style to SVG for baseline alignment.
    /// Enables inline math to align with surrounding text.
    /// Default: false (opt-in)
    #[config(default = "false")]
    pub baseline_align: bool,
}

impl Default for SvgConfig {
    fn default() -> Self {
        Self {
            external: false,
            format: SvgFormat::SVG,
            converter: SvgConverter::Builtin,
            dpi: 96.0,
            threshold: "0B".to_string(),
            expand_viewbox: true,
            baseline_align: false,
        }
    }
}

impl SvgConfig {
    /// Check if SVG should be embedded in HTML.
    pub fn is_embedded(&self) -> bool {
        !self.external
    }

    /// Check if SVG should be kept as SVG (no rasterization).
    pub fn is_svg_output(&self) -> bool {
        self.external && self.format == SvgFormat::SVG
    }

    /// Check if SVG needs rasterization.
    pub fn needs_rasterization(&self) -> bool {
        self.external && self.format.needs_rasterization()
    }

    /// Parse threshold string to bytes.
    pub fn threshold_bytes(&self) -> usize {
        parse_size_string(&self.threshold)
    }

    /// Validate SVG configuration.
    ///
    /// # Checks
    /// - If rasterization is needed and converter is external (magick/ffmpeg),
    ///   the command must be installed.
    pub fn validate(&self, diag: &mut crate::config::ConfigDiagnostics) {
        // Only check if external conversion is needed
        if !self.needs_rasterization() {
            return;
        }

        match &self.converter {
            SvgConverter::Builtin => {}
            SvgConverter::Magick => {
                if which::which("magick").is_err() {
                    diag.error_with_hint(
                        Self::FIELDS.converter,
                        "`magick` command not found",
                        format!(
                            "install ImageMagick or set {} = \"builtin\"",
                            Self::FIELDS.converter
                        ),
                    );
                }
            }
            SvgConverter::Ffmpeg => {
                if which::which("ffmpeg").is_err() {
                    diag.error_with_hint(
                        Self::FIELDS.converter,
                        "`ffmpeg` command not found",
                        format!(
                            "install FFmpeg or set {} = \"builtin\"",
                            Self::FIELDS.converter
                        ),
                    );
                }
            }
        }
    }
}

/// Parse size string (e.g., "10KB") to bytes
fn parse_size_string(s: &str) -> usize {
    let s = s.trim().to_uppercase();
    if s.ends_with("MB") {
        s.trim_end_matches("MB")
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
            * 1024
            * 1024
    } else if s.ends_with("KB") {
        s.trim_end_matches("KB")
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
            * 1024
    } else if s.ends_with('B') {
        s.trim_end_matches('B').trim().parse().unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;

    #[test]
    fn test_defaults() {
        let config = test_parse_config("");
        assert!(!config.build.svg.external);
        assert_eq!(config.build.svg.format, SvgFormat::SVG);
        assert_eq!(config.build.svg.converter, SvgConverter::Builtin);
        assert_eq!(config.build.svg.dpi, 96.0);
        assert!(config.build.svg.expand_viewbox);
        assert!(!config.build.svg.baseline_align);
    }

    #[test]
    fn test_format_parsing() {
        let cases = [
            ("svg", SvgFormat::SVG),
            ("png", SvgFormat::PNG),
            ("jpg", SvgFormat::JPG),
            ("webp", SvgFormat::WEBP),
        ];
        for (input, expected) in cases {
            let config = test_parse_config(&format!("[build.svg]\nformat = \"{input}\""));
            assert_eq!(config.build.svg.format, expected, "failed for {input}");
        }
    }

    #[test]
    fn test_converter_parsing() {
        let cases = [
            ("builtin", SvgConverter::Builtin),
            ("magick", SvgConverter::Magick),
            ("ffmpeg", SvgConverter::Ffmpeg),
        ];
        for (input, expected) in cases {
            let config = test_parse_config(&format!("[build.svg]\nconverter = \"{input}\""));
            assert_eq!(config.build.svg.converter, expected, "failed for {input}");
        }
    }

    #[test]
    fn test_format_extension() {
        assert_eq!(SvgFormat::SVG.extension(), "svg");
        assert_eq!(SvgFormat::PNG.extension(), "png");
        assert_eq!(SvgFormat::JPG.extension(), "jpg");
        assert_eq!(SvgFormat::WEBP.extension(), "webp");
    }

    #[test]
    fn test_needs_rasterization() {
        assert!(!SvgFormat::SVG.needs_rasterization());
        assert!(SvgFormat::PNG.needs_rasterization());
    }

    #[test]
    fn test_is_embedded() {
        let config = test_parse_config("");
        assert!(config.build.svg.is_embedded());

        let config = test_parse_config("[build.svg]\nexternal = true");
        assert!(!config.build.svg.is_embedded());
    }

    #[test]
    fn test_is_svg_output() {
        let config = test_parse_config("[build.svg]\nexternal = true\nformat = \"svg\"");
        assert!(config.build.svg.is_svg_output());

        let config = test_parse_config("[build.svg]\nexternal = true\nformat = \"png\"");
        assert!(!config.build.svg.is_svg_output());

        let config = test_parse_config("[build.svg]\nexternal = false\nformat = \"svg\"");
        assert!(!config.build.svg.is_svg_output());
    }

    #[test]
    fn test_parse_size_string() {
        assert_eq!(parse_size_string("0B"), 0);
        assert_eq!(parse_size_string("100B"), 100);
        assert_eq!(parse_size_string("10KB"), 10 * 1024);
        assert_eq!(parse_size_string("1MB"), 1024 * 1024);
        assert_eq!(parse_size_string("  5kb  "), 5 * 1024);
        assert_eq!(parse_size_string("invalid"), 0);
    }

    #[test]
    fn test_threshold_bytes() {
        let config = test_parse_config("[build.svg]\nthreshold = \"10KB\"");
        assert_eq!(config.build.svg.threshold_bytes(), 10 * 1024);
    }
}
