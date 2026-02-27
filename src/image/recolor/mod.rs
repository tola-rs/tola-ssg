//! Image recolor utilities for theme adaptation.
//!
//! This module contains pure recolor logic shared by:
//! - Runtime SVG filter generation (theme.recolor = "static")
//! - Other image-related recolor entrypoints

use std::collections::HashMap;

/// Parse hex color to RGB values (0.0-1.0).
pub fn parse_hex_color(hex: &str) -> Option<(f32, f32, f32)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
    Some((r, g, b))
}

/// Generate SVG filter for a specific color.
///
/// Uses luminance-based switching:
/// - black maps to target color
/// - white maps to black for light targets, or white for dark targets
pub fn generate_filter(id: &str, hex: &str) -> Option<String> {
    let (r, g, b) = parse_hex_color(hex)?;

    // Luminance-based B value: light target -> white becomes black
    let target_lum = 0.299 * r + 0.587 * g + 0.114 * b;
    let b_val = if target_lum > 0.5 { 0.0 } else { 1.0 };

    Some(format!(
        r#"<filter id="{id}" color-interpolation-filters="sRGB">
      <feColorMatrix type="matrix" values=".33 .33 .33 0 0
                                           .33 .33 .33 0 0
                                           .33 .33 .33 0 0
                                            0   0   0  1 0"/>
      <feComponentTransfer>
        <feFuncR type="table" tableValues="{r:.3} {b_val:.3}"/>
        <feFuncG type="table" tableValues="{g:.3} {b_val:.3}"/>
        <feFuncB type="table" tableValues="{b:.3} {b_val:.3}"/>
      </feComponentTransfer>
    </filter>"#
    ))
}

/// Generate static mode SVG with all theme filters.
pub fn generate_static_svg(list: &HashMap<String, String>) -> String {
    let mut filters = Vec::new();
    for (name, color) in list {
        if let Some(filter) = generate_filter(&format!("tola-recolor-{name}"), color) {
            filters.push(filter);
        }
    }
    format!(
        r#"<svg style="display:none" aria-hidden="true">
  <defs>
    {}
  </defs>
</svg>"#,
        filters.join("\n    ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color_valid() {
        let (r, g, b) = parse_hex_color("#88c0d0").unwrap();
        assert!((r - 0.533).abs() < 0.01);
        assert!((g - 0.753).abs() < 0.01);
        assert!((b - 0.816).abs() < 0.01);
    }

    #[test]
    fn test_parse_hex_color_invalid() {
        assert!(parse_hex_color("#fff").is_none());
        assert!(parse_hex_color("zzzzzz").is_none());
    }

    #[test]
    fn test_generate_filter_invalid_hex() {
        assert!(generate_filter("test", "#fff").is_none());
    }

    #[test]
    fn test_generate_static_svg_contains_theme_filters() {
        let list = HashMap::from([
            ("light".to_string(), "#000000".to_string()),
            ("dark".to_string(), "#ffffff".to_string()),
        ]);

        let svg = generate_static_svg(&list);
        assert!(svg.contains("tola-recolor-light"));
        assert!(svg.contains("tola-recolor-dark"));
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }
}
