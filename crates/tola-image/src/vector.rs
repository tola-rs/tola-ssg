//! Vector geometry and SVG renderer identity for image recipes.

use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::recipe::{
    ImageRecipe, ImageRequestError, OutputFormat, PixelRecipe, ResizeOperation, ResizeOptions,
    encoding_for, requested_height, requested_width, validate_request,
};

/// Bump when the SVG renderer stack changes pixels for the same source and recipe.
pub const SVG_RENDERER_REVISION: u16 = 1;

/// One exact coordinate or extent, compared by its bits so recipes stay `Eq` and `Hash`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(into = "u64", try_from = "u64")]
struct Exact(f64);

impl Exact {
    fn checked(val: f64) -> Result<Self, ImageRequestError> {
        if !val.is_finite() {
            return Err(ImageRequestError::SvgGeometryNotFinite);
        }
        Ok(Self(if val == 0.0 { 0.0 } else { val }))
    }

    const fn get(self) -> f64 {
        self.0
    }
}

impl From<Exact> for u64 {
    fn from(val: Exact) -> Self {
        val.0.to_bits()
    }
}

impl TryFrom<u64> for Exact {
    type Error = ImageRequestError;

    fn try_from(bits: u64) -> Result<Self, Self::Error> {
        Self::checked(f64::from_bits(bits))
    }
}

impl PartialEq for Exact {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Exact {}

impl Hash for Exact {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

/// The source rectangle a vector recipe renders, in the SVG's own pixel coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorGeometry {
    source: (Exact, Exact),
    crop: (Exact, Exact, Exact, Exact),
}

impl VectorGeometry {
    fn new(src: (f64, f64), crop: (f64, f64, f64, f64)) -> Result<Self, ImageRequestError> {
        let geom = Self {
            source: (Exact::checked(src.0)?, Exact::checked(src.1)?),
            crop: (
                Exact::checked(crop.0)?,
                Exact::checked(crop.1)?,
                Exact::checked(crop.2)?,
                Exact::checked(crop.3)?,
            ),
        };
        geom.validate()?;
        Ok(geom)
    }

    /// The source's exact pixel size.
    pub fn source(self) -> (f64, f64) {
        (self.source.0.get(), self.source.1.get())
    }

    /// The rendered source rectangle: left, top, width, height.
    pub fn crop(self) -> (f64, f64, f64, f64) {
        (
            self.crop.0.get(),
            self.crop.1.get(),
            self.crop.2.get(),
            self.crop.3.get(),
        )
    }

    pub(super) fn validate(self) -> Result<(), ImageRequestError> {
        let (src_w, src_h) = self.source();
        let (x, y, w, h) = self.crop();
        if !(src_w > 0.0 && src_h > 0.0 && x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0) {
            return Err(ImageRequestError::SvgGeometryInvalid);
        }
        let slack_x = 1e-9 * src_w.max(1.0);
        let slack_y = 1e-9 * src_h.max(1.0);
        if !(x + w <= src_w + slack_x && y + h <= src_h + slack_y) {
            return Err(ImageRequestError::SvgCropOutsideSource);
        }
        Ok(())
    }
}

/// The SVG renderer identity: renderer revision plus the parsed source hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SvgRender {
    revision: u16,
    #[serde(with = "hash_hex")]
    hash: u128,
}

impl SvgRender {
    /// The identity for a source parsed by the current renderer.
    pub fn new(hash: u128) -> Self {
        Self {
            revision: SVG_RENDERER_REVISION,
            hash,
        }
    }

    /// The renderer revision this identity was recorded with.
    pub const fn revision(self) -> u16 {
        self.revision
    }

    /// The parsed source hash: bytes plus the fonts the parse used.
    pub const fn hash(self) -> u128 {
        self.hash
    }

    pub(super) fn validate(self) -> Result<(), ImageRequestError> {
        if self.revision != SVG_RENDERER_REVISION {
            return Err(ImageRequestError::SvgRendererUnsupported);
        }
        Ok(())
    }
}

mod hash_hex {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(val: &u128, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&format!("{val:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<u128, D::Error> {
        let text = String::deserialize(de)?;
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(D::Error::custom(
                "the SVG renderer hash must be 32 lowercase hexadecimal digits",
            ));
        }
        u128::from_str_radix(&text, 16).map_err(D::Error::custom)
    }
}

impl ImageRecipe {
    /// Resolve one SVG request against the source's exact pixel size and the renderer identity.
    ///
    /// The format must be explicit: an SVG has no source pixel format to infer from, and a raster
    /// derivative is never the silent default.
    pub fn resolve_svg(
        src: (f64, f64),
        opts: ResizeOptions,
        render: SvgRender,
    ) -> Result<Self, ImageRequestError> {
        if !(src.0.is_finite() && src.1.is_finite() && src.0 > 0.0 && src.1 > 0.0) {
            return Err(ImageRequestError::SvgIntrinsicSize);
        }
        if opts.format == OutputFormat::Auto {
            return Err(ImageRequestError::SvgFormatRequired);
        }
        validate_request(opts)?;
        let mut crop = (0.0, 0.0, src.0, src.1);
        let (w, h) = match opts.operation {
            ResizeOperation::Scale => (requested_width(&opts)?, requested_height(&opts)?),
            ResizeOperation::FitWidth => {
                let w = requested_width(&opts)?;
                (w, scaled_pixels_f64(src.1, f64::from(w), src.0)?)
            }
            ResizeOperation::FitHeight => {
                let h = requested_height(&opts)?;
                (scaled_pixels_f64(src.0, f64::from(h), src.1)?, h)
            }
            ResizeOperation::Fit => {
                let w = requested_width(&opts)?;
                let h = requested_height(&opts)?;
                if src.0 <= f64::from(w) && src.1 <= f64::from(h) {
                    (rounded_pixel(src.0)?, rounded_pixel(src.1)?)
                } else if f64::from(w) * src.1 <= f64::from(h) * src.0 {
                    (w, scaled_pixels_f64(src.1, f64::from(w), src.0)?.min(h))
                } else {
                    (scaled_pixels_f64(src.0, f64::from(h), src.1)?.min(w), h)
                }
            }
            ResizeOperation::Fill => {
                let w = requested_width(&opts)?;
                let h = requested_height(&opts)?;
                let scale = (f64::from(w) / src.0).max(f64::from(h) / src.1);
                let crop_w = f64::from(w) / scale;
                let crop_h = f64::from(h) / scale;
                let (horiz, vert) = opts.anchor.alignments();
                crop = (
                    horiz.offset(src.0, crop_w),
                    vert.offset(src.1, crop_h),
                    crop_w,
                    crop_h,
                );
                (w, h)
            }
        };
        let geom = VectorGeometry::new(src, crop)?;
        let encoding = encoding_for(opts.format, opts.quality);
        let mut recipe = Self {
            pixels: PixelRecipe {
                width: w,
                height: h,
                crop: None,
                vector: Some(geom),
                filter: opts.filter,
                background: opts.background,
            },
            encoding,
            svg: Some(render),
        };
        recipe.pixels.normalize_filter();
        recipe.validate_structure()?;
        recipe.validate_colors(true)?;
        Ok(recipe)
    }

    /// Whether the parsed SVG with this size and hash can be rendered by this recipe.
    pub fn validate_svg(&self, w: f64, h: f64, hash: u128) -> Result<(), ImageRequestError> {
        let Some(vector) = self.pixels.vector else {
            return Err(ImageRequestError::NotAVectorRecipe);
        };
        let Some(svg) = self.svg else {
            return Err(ImageRequestError::NoSvgRendererIdentity);
        };
        let (src_w, src_h) = vector.source();
        let tol = 1e-6 * w.abs().max(1.0);
        if svg.hash() != hash || (src_w - w).abs() > tol || (src_h - h).abs() > tol {
            return Err(ImageRequestError::SourceChanged);
        }
        self.validate_colors(true)
    }
}

/// Nearest integer at least one pixel, refusing values that cannot be a pixel count.
fn rounded_pixel(val: f64) -> Result<u32, ImageRequestError> {
    let rounded = val.round().max(1.0);
    if rounded > f64::from(u32::MAX) {
        return Err(ImageRequestError::ResizedTooLarge);
    }
    Ok(rounded as u32)
}

fn scaled_pixels_f64(dim: f64, num: f64, den: f64) -> Result<u32, ImageRequestError> {
    rounded_pixel(dim * num / den)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{CropAnchor, ResizeFilter};

    fn vector_options(op: ResizeOperation, w: Option<u32>, h: Option<u32>) -> ResizeOptions {
        ResizeOptions {
            width: w,
            height: h,
            operation: op,
            anchor: CropAnchor::Center,
            format: OutputFormat::Png,
            quality: None,
            filter: ResizeFilter::Lanczos3,
            background: None,
        }
    }

    #[test]
    fn svg_fill_anchor() {
        let mut req = vector_options(ResizeOperation::Fill, Some(50), Some(50));
        req.anchor = CropAnchor::BottomRight;
        let recipe = ImageRecipe::resolve_svg((100.0, 50.0), req, SvgRender::new(9)).unwrap();
        let (x, y, w, h) = recipe.vector_geometry().unwrap().crop();
        assert_eq!((x, y), (50.0, 0.0));
        assert_eq!((w, h), (50.0, 50.0));

        let mut req = vector_options(ResizeOperation::Fit, Some(50), Some(50));
        req.anchor = CropAnchor::BottomRight;
        assert!(ImageRecipe::resolve_svg((100.0, 50.0), req, SvgRender::new(9)).is_err());
    }

    #[test]
    fn vector_display_geometry() {
        let src = (300.0, 380.0);
        let render = SvgRender::new(7);
        for (op, w, h, expected) in [
            (ResizeOperation::Scale, Some(150), Some(150), (150, 150)),
            (ResizeOperation::FitWidth, Some(100), None, (100, 127)),
            (ResizeOperation::FitHeight, None, Some(150), (118, 150)),
            (ResizeOperation::Fit, Some(150), Some(150), (118, 150)),
            (ResizeOperation::Fill, Some(150), Some(150), (150, 150)),
        ] {
            let recipe = ImageRecipe::resolve_svg(src, vector_options(op, w, h), render).unwrap();
            assert_eq!((recipe.width(), recipe.height()), expected);
            assert!(recipe.vector_geometry().is_some());
            assert_eq!(recipe.svg_render(), Some(render));
        }
        let fill = ImageRecipe::resolve_svg(
            src,
            vector_options(ResizeOperation::Fill, Some(150), Some(150)),
            render,
        )
        .unwrap();
        let (x, y, w, h) = fill.vector_geometry().unwrap().crop();
        assert_eq!((x, y), (0.0, 40.0));
        assert!((w - 300.0).abs() < 1e-9 && (h - 300.0).abs() < 1e-9);
    }

    #[test]
    fn vector_rejects_auto_format() {
        let mut req = vector_options(ResizeOperation::Scale, Some(10), Some(10));
        req.format = OutputFormat::Auto;
        let err = ImageRecipe::resolve_svg((10.0, 10.0), req, SvgRender::new(1)).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot choose a format for an SVG"),
            "{err}"
        );
    }

    #[test]
    fn vector_unused_filter() {
        let render = SvgRender::new(1);
        let mut tri = vector_options(ResizeOperation::FitWidth, Some(100), None);
        tri.filter = ResizeFilter::Triangle;
        let lanczos = ImageRecipe::resolve_svg(
            (300.0, 380.0),
            vector_options(ResizeOperation::FitWidth, Some(100), None),
            render,
        )
        .unwrap();
        assert_eq!(
            lanczos,
            ImageRecipe::resolve_svg((300.0, 380.0), tri, render).unwrap()
        );
    }

    #[test]
    fn vector_validation_matches_src() {
        let recipe = ImageRecipe::resolve_svg(
            (10.0, 10.0),
            vector_options(ResizeOperation::Scale, Some(5), Some(5)),
            SvgRender::new(9),
        )
        .unwrap();
        recipe.validate_svg(10.0, 10.0, 9).unwrap();
        assert!(recipe.validate_svg(11.0, 10.0, 9).is_err());
        assert!(recipe.validate_svg(10.0, 10.0, 8).is_err());
    }
}
