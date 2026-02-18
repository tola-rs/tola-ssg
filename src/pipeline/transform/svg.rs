//! SVG processor (Indexed -> Indexed).
//!
//! Processes SVG elements based on build mode:
//! - **serve mode**: Optimize inline SVG (viewBox adjustment)
//! - **build mode**: Optimize + extract to external files (when `external = true`)
//!
//! # SVG Representation in VDOM
//!
//! SVG elements from Typst are stored as:
//! ```text
//! Element {
//!     tag: "svg",
//!     attrs: { viewBox: "...", width: "...", ... },
//!     children: [Text::raw("<path>...</path><g>...</g>")]
//! }
//! ```
//!
//! The inner content (paths, groups, etc.) is a raw text node, not parsed elements.
//! This allows efficient reconstruction of the full SVG string.

use std::path::PathBuf;

use anyhow::Result;
use tola_vdom::prelude::*;

use crate::compiler::family::{Indexed, TolaSite::FamilyKind};
use crate::compiler::page::PageRoute;
use crate::config::SiteConfig;
use crate::core::BuildMode;
use crate::image::svg::{ExtractContext, OptimizeOptions, extract_svg_to_file, optimize_svg};

/// Processes SVG elements in Indexed VDOM
///
/// In serve mode, SVGs are optimized but kept inline for hot reload compatibility
/// In build mode with `external = true`, SVGs are extracted to `.tola/` subdirectory
pub struct SvgTransform<'a> {
    config: &'a SiteConfig,
    route: &'a PageRoute,
    mode: BuildMode,
}

impl<'a> SvgTransform<'a> {
    pub fn new(config: &'a SiteConfig, route: &'a PageRoute, mode: BuildMode) -> Self {
        Self {
            config,
            route,
            mode,
        }
    }

    /// Check if SVG should be extracted to external file.
    #[inline]
    fn should_extract(&self) -> bool {
        self.config.build.svg.external && !self.mode.is_dev()
    }

    /// Get output directory for extracted SVG files.
    #[inline]
    fn output_dir(&self) -> &PathBuf {
        &self.route.output_dir
    }

    /// Create extraction context from config.
    fn extract_context(&self) -> ExtractContext {
        ExtractContext::new(
            self.output_dir().clone(),
            self.config.build.svg.format.clone(),
            self.config.build.svg.converter.clone(),
            self.config.build.svg.dpi,
            self.config.build.svg.threshold_bytes(),
            self.config.build.svg.expand_viewbox,
        )
    }

    /// Reconstruct full SVG string from element.
    ///
    /// Combines attributes and inner text content into a complete SVG.
    fn reconstruct_svg(&self, elem: &Element<Indexed>) -> String {
        let mut svg = String::with_capacity(1024);
        svg.push_str("<svg");

        // Add attributes
        for (key, value) in elem.attrs.iter() {
            svg.push(' ');
            svg.push_str(key);
            svg.push_str("=\"");
            // Escape attribute value
            for c in value.chars() {
                match c {
                    '"' => svg.push_str("&quot;"),
                    '&' => svg.push_str("&amp;"),
                    '<' => svg.push_str("&lt;"),
                    '>' => svg.push_str("&gt;"),
                    _ => svg.push(c),
                }
            }
            svg.push('"');
        }
        svg.push('>');

        // Add inner content (raw text from children)
        svg.push_str(&elem.text_content());

        svg.push_str("</svg>");
        svg
    }

    /// Optimize SVG content inline (for serve mode).
    fn optimize_inline(&self, elem: &mut Element<Indexed>) -> Result<()> {
        let svg_content = self.reconstruct_svg(elem);
        if svg_content.len() < 10 {
            // Too small to be valid SVG
            return Ok(());
        }

        let options = OptimizeOptions {
            dpi: self.config.build.svg.dpi,
            expand_viewbox: self.config.build.svg.expand_viewbox,
        };

        let optimized = optimize_svg(svg_content.as_bytes(), &options)?;

        // Update viewBox attribute from optimized SVG
        if let Some(viewbox) = extract_viewbox_from_bytes(&optimized.data) {
            elem.set_attr("viewBox", viewbox);
        }

        Ok(())
    }

    /// Extract SVG to external file (for build mode).
    fn extract_to_file(&self, elem: &mut Element<Indexed>) -> Result<()> {
        let svg_content = self.reconstruct_svg(elem);
        if svg_content.len() < 10 {
            return Ok(());
        }

        let ctx = self.extract_context();

        // Check threshold - small SVGs stay inline
        if ctx.should_inline(svg_content.len()) {
            return self.optimize_inline(elem);
        }

        // Extract to file
        let result = extract_svg_to_file(svg_content.as_bytes(), &ctx)?;

        // Replace <svg> with <img> pointing to extracted file
        self.replace_with_img(elem, &result.relative_path);

        Ok(())
    }

    /// Replace SVG element with img element pointing to extracted file.
    fn replace_with_img(&self, elem: &mut Element<Indexed>, src: &str) {
        // Clear children (SVG content no longer needed)
        elem.children.clear();

        // Change tag from svg to img
        elem.tag = "img".into();

        // Remove SVG-specific attributes
        elem.remove_attr("viewBox");
        elem.remove_attr("xmlns");
        elem.remove_attr("xmlns:xlink");

        // Set img attributes
        elem.set_attr("src", src.to_string());
        elem.set_attr("loading", "lazy");
    }
}

impl Transform<Indexed> for SvgTransform<'_> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        let should_extract = self.should_extract();

        doc.modify_by::<FamilyKind::Svg, _>(|elem| {
            // Only process root <svg> elements (not nested SVG elements like <path>)
            if !elem.is_tag("svg") {
                return;
            }

            let result = if should_extract {
                self.extract_to_file(elem)
            } else {
                self.optimize_inline(elem)
            };

            if let Err(e) = result {
                // Log error but don't fail the entire transform
                eprintln!("SVG processing error: {}", e);
            }
        });

        doc
    }
}

/// Extract viewBox attribute value from SVG bytes
fn extract_viewbox_from_bytes(svg: &[u8]) -> Option<String> {
    let svg_str = std::str::from_utf8(svg).ok()?;
    let start = svg_str.find("viewBox=\"")? + 9;
    let end = start + svg_str[start..].find('"')?;
    Some(svg_str[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_viewbox_from_bytes() {
        let svg = br#"<svg viewBox="0 0 100 200" width="100">"#;
        assert_eq!(
            extract_viewbox_from_bytes(svg),
            Some("0 0 100 200".to_string())
        );

        let svg = br#"<svg width="100" viewBox="0 -5 100 209">"#;
        assert_eq!(
            extract_viewbox_from_bytes(svg),
            Some("0 -5 100 209".to_string())
        );

        let svg = br#"<svg width="100">"#;
        assert_eq!(extract_viewbox_from_bytes(svg), None);
    }

    #[test]
    fn test_should_extract() {
        let config = SiteConfig::default();
        let route = PageRoute::default();

        // Development mode: never extract
        let transform = SvgTransform::new(&config, &route, BuildMode::DEVELOPMENT);
        assert!(!transform.should_extract());

        // Production mode with external=false: don't extract
        let mut config_no_extract = SiteConfig::default();
        config_no_extract.build.svg.external = false;
        let transform = SvgTransform::new(&config_no_extract, &route, BuildMode::PRODUCTION);
        assert!(!transform.should_extract());

        // Production mode with external=true: extract
        let mut config_extract = SiteConfig::default();
        config_extract.build.svg.external = true;
        let transform = SvgTransform::new(&config_extract, &route, BuildMode::PRODUCTION);
        assert!(transform.should_extract());
    }
}
