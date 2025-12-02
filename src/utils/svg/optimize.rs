use anyhow::{Context, Result};
use crate::config::SiteConfig;

/// Optimize SVG using usvg, returning optimized bytes and dimensions.
pub fn optimize_svg(content: &[u8], config: &SiteConfig) -> Result<(Vec<u8>, (f32, f32))> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
