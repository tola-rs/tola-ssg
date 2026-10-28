//! Deterministic SVG rasterization through Typst's own usvg/resvg stack.
//!
//! An SVG has no pixel grid, so a derivative request against one is a rasterization, not a
//! resampling. Parsing reuses [`SvgImage`], so text resolves against the compiler's font book and
//! linked images follow the same policy as `image()`; rendering uses `resvg` into a target pixmap.
//! The output is row-major RGBA8 with straight alpha, ready for the image encoders.

use resvg::tiny_skia::{Pixmap, Transform};
use thiserror::Error;
use typst::World;
use typst::comemo::Tracked;
use typst::diag::LoadedWithin;
use typst::foundations::{Bytes, StyleChain};
use typst::loading::{LoadSource, Loaded};
use typst::syntax::{FileId, Span, Spanned};
use typst::utils::hash128;
use typst::visualize::SvgImage;

/// One parsed SVG source.
#[derive(Clone)]
pub struct SvgSource {
    image: SvgImage,
}

/// A source rectangle to render, in the SVG's own pixel coordinates.
#[derive(Clone, Copy, Debug)]
pub struct SvgCrop {
    /// The left edge.
    pub x: f64,
    /// The top edge.
    pub y: f64,
    /// The rectangle's width.
    pub width: f64,
    /// The rectangle's height.
    pub height: f64,
}

/// Why an SVG could not be parsed or rendered.
#[derive(Debug, Clone, Error)]
pub enum SvgError {
    /// The SVG links an image that is not a data URI.
    #[error("it links `{href}`; inline the image as a data URI")]
    ExternalImage {
        /// The refused reference.
        href: String,
    },
    /// The SVG did not parse.
    #[error("{message}")]
    Load {
        /// The parser's reason.
        message: String,
    },
    /// The requested raster cannot be allocated.
    #[error("{message}")]
    Render {
        /// What could not be allocated.
        message: String,
    },
}

impl SvgSource {
    /// Parse `bytes`, resolving text and linked images through `world`.
    ///
    /// Linked images other than data URIs are refused: their reads would fall outside the
    /// derivative request's read evidence.
    pub fn parse(
        bytes: Bytes,
        world: Tracked<dyn World + '_>,
        file: FileId,
    ) -> Result<Self, SvgError> {
        refuse_linked_images(&bytes)?;
        let loaded = Loaded::new(
            Spanned::new(LoadSource::Path(file), Span::detached()),
            bytes.clone(),
        );
        let families = default_families(world);
        let family_refs = families.iter().map(String::as_str).collect::<Vec<_>>();
        let image = SvgImage::with_fonts_images(bytes, world, &family_refs, Some(file)).map_err(
            |error| SvgError::Load {
                message: error.within(&loaded).message.to_string(),
            },
        )?;
        Ok(Self { image })
    }

    /// The SVG's intrinsic width in pixels, at usvg's 96 DPI.
    pub fn width(&self) -> f64 {
        self.image.width()
    }

    /// The SVG's intrinsic height in pixels, at usvg's 96 DPI.
    pub fn height(&self) -> f64 {
        self.image.height()
    }

    /// A hash of the SVG bytes and the fonts the parse actually used.
    pub fn hash(&self) -> u128 {
        hash128(&self.image)
    }

    /// Render into `width × height` straight-alpha RGBA8 pixels.
    ///
    /// `crop` selects the source rectangle to draw; `None` draws the whole source. The caller
    /// maps the recipe's operation (`fit`, `fill`, …) to this rectangle and target size.
    pub fn render(
        &self,
        width: u32,
        height: u32,
        crop: Option<SvgCrop>,
    ) -> Result<Vec<u8>, SvgError> {
        let crop = crop.unwrap_or_else(|| SvgCrop {
            x: 0.0,
            y: 0.0,
            width: self.width(),
            height: self.height(),
        });
        if width == 0 || height == 0 || !(crop.width > 0.0 && crop.height > 0.0) {
            return Err(SvgError::Render {
                message: "the raster size must be positive".into(),
            });
        }
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4));
        if pixels.is_none_or(|pixels| pixels > usize::MAX as u64) {
            return Err(SvgError::Render {
                message: "the raster is too large to allocate".into(),
            });
        }
        let mut pixmap = Pixmap::new(width, height).ok_or_else(|| SvgError::Render {
            message: "the raster is too large to allocate".into(),
        })?;
        let scale_x = width as f32 / crop.width as f32;
        let scale_y = height as f32 / crop.height as f32;
        let transform = Transform::from_row(
            scale_x,
            0.0,
            0.0,
            scale_y,
            -crop.x as f32 * scale_x,
            -crop.y as f32 * scale_y,
        );
        resvg::render(self.image.tree(), transform, &mut pixmap.as_mut());
        Ok(straight_rgba(&pixmap))
    }
}

/// Whether `bytes` is an SVG document, judged by its root element rather than its name.
pub fn looks_like_svg(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(document) = roxmltree::Document::parse(text) else {
        return false;
    };
    let root = document.root_element();
    root.tag_name().name() == "svg"
        && root
            .tag_name()
            .namespace()
            .is_none_or(|namespace| namespace == "http://www.w3.org/2000/svg")
}

/// The default text families for a bitmap derivative: the compiler's own font chain.
fn default_families(world: Tracked<dyn World + '_>) -> Vec<String> {
    let library = world.library();
    let styles = StyleChain::new(&library.styles);
    typst::text::families(styles)
        .map(|family| family.as_str().to_owned())
        .collect()
}

/// Refuse linked images that would be read outside the derivative request's evidence.
fn refuse_linked_images(bytes: &[u8]) -> Result<(), SvgError> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(());
    };
    let Ok(document) = roxmltree::Document::parse(text) else {
        return Ok(());
    };
    for node in document.descendants().filter(|node| node.is_element()) {
        if !matches!(node.tag_name().name(), "image" | "use" | "feImage") {
            continue;
        }
        let Some(href) = node
            .attribute("href")
            .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))
        else {
            continue;
        };
        let href = href.trim();
        if href.starts_with('#') || href.to_ascii_lowercase().starts_with("data:") {
            continue;
        }
        return Err(SvgError::ExternalImage {
            href: href.to_owned(),
        });
    }
    Ok(())
}

/// The pixmap's straight-alpha bytes; tiny-skia stores premultiplied pixels.
fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::BundleCancellation;
    use crate::TypstWorld;
    use crate::file_id;
    use typst::comemo::Track;

    fn world() -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, "").unwrap();
        let world = TypstWorld::builder(&main, directory.path())
            .no_fonts()
            .with_local_cache()
            .build(&BundleCancellation::default())
            .unwrap();
        (directory, world)
    }

    fn parse(world: &TypstWorld, svg: &str) -> Result<SvgSource, SvgError> {
        SvgSource::parse(
            Bytes::new(svg.as_bytes().to_vec()),
            (world as &dyn World).track(),
            file_id("logo.svg"),
        )
    }

    #[test]
    fn linked_image_is_refused() {
        let (_directory, world) = world();
        let refused = parse(
            &world,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="photo.png"/></svg>"##,
        )
        .err().expect("the linked image is refused");
        assert!(
            matches!(&refused, SvgError::ExternalImage { href } if href == "photo.png"),
            "{refused:?}"
        );
        let xlink = parse(
            &world,
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image xlink:href="photo.png"/></svg>"##,
        )
        .err().expect("the linked image is refused");
        assert!(matches!(xlink, SvgError::ExternalImage { .. }), "{xlink:?}");
        parse(
            &world,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><use href="#shape"/></svg>"##,
        )
        .unwrap();
    }

    #[test]
    fn render_is_deterministic() {
        let (_directory, world) = world();
        let source = parse(
            &world,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##,
        )
        .unwrap();
        assert_eq!(
            source.render(4, 4, None).unwrap(),
            source.render(4, 4, None).unwrap()
        );
    }
}
