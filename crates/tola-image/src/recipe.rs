use serde::{Deserialize, Serialize};

use super::{ImageFormat, ImageMetadata, check_dimensions};
pub use crate::vector::{SvgRender, VectorGeometry};

/// Why one resize request cannot become a recipe.
#[derive(Debug, thiserror::Error)]
pub enum ImageRequestError {
    /// The source has no positive pixel dimensions.
    #[error("image dimensions must be positive")]
    InvalidDimensions,
    /// The request names a vector recipe where resampling was asked for.
    #[error("vector recipes are rendered, not resampled")]
    VectorRecipeNotResampled,
    /// The stored crop extends past the source.
    #[error("the resize crop is outside the source image")]
    CropOutsideSource,
    /// The stored crop is neither the whole source nor one anchored fill crop.
    #[error("image crop must be the complete source or one of its anchored fill crops")]
    CropNotAnchoredFill,
    /// The crop coordinates cannot be added without overflowing.
    #[error("image crop coordinates overflow")]
    CropCoordinatesOverflow,
    /// The recipe names a crop and a vector, or neither.
    #[error("the image recipe needs exactly one source region")]
    SourceRegionCount,
    /// The stored recipe mixes raster crop and vector geometry.
    #[error("the image recipe mixes raster and vector source geometry")]
    MixedSourceGeometry,
    /// A vector coordinate is not a finite number.
    #[error("the SVG source geometry is not finite")]
    SvgGeometryNotFinite,
    /// The vector source or crop is not a positive rectangle.
    #[error("the SVG source geometry is invalid")]
    SvgGeometryInvalid,
    /// The vector crop extends past the source.
    #[error("the SVG crop is outside the source")]
    SvgCropOutsideSource,
    /// The recorded SVG renderer revision is not the current one.
    #[error("the saved copy of this SVG was made by another Tola version; run the build again")]
    SvgRendererUnsupported,
    /// The parsed SVG reports no usable intrinsic size.
    #[error("the SVG has no valid intrinsic size")]
    SvgIntrinsicSize,
    /// An SVG derivative must name its raster format.
    #[error(
        "`resize-image` cannot choose a format for an SVG; pass `format: \"png\"`, `\"jpg\"` (or `\"jpeg\"`), or `\"webp\"` to rasterize it, or use `image()` for vector display"
    )]
    SvgFormatRequired,
    /// JPEG quality is outside 1 to 100.
    #[error("`quality` must be 1 to 100 for JPEG output")]
    JpegQuality,
    /// JPEG cannot encode this frame size.
    #[error("JPEG output cannot exceed 65535 pixels wide or tall; reduce `width` or `height`")]
    JpegDimensions,
    /// WebP quality is outside 0 to 100.
    #[error("`quality` must be 0 to 100 for WebP output")]
    WebpQuality,
    /// WebP cannot encode this frame size.
    #[error("WebP output cannot exceed 16383 pixels wide or tall; reduce `width` or `height`")]
    WebpDimensions,
    /// `anchor` was supplied for an operation that never crops.
    #[error("`anchor` applies only to `op: \"fill\"`; the other operations keep the whole source")]
    AnchorWithoutFill,
    /// A requested dimension is zero.
    #[error("`width` and `height` must be positive pixel counts")]
    NonPositiveDimensions,
    /// An operation needs a dimension the request omits.
    #[error("`resize-image` needs `{dimension}` for this `op`; pass `{dimension}`")]
    MissingDimension {
        /// The missing option, `width` or `height`.
        dimension: &'static str,
    },
    /// The scaled pixel count cannot be addressed.
    #[error("the resized image would be too large; use a smaller `width` and `height`")]
    ResizedTooLarge,
    /// The recipe is not a vector recipe.
    #[error("the image recipe is not a vector recipe")]
    NotAVectorRecipe,
    /// The recipe carries no SVG renderer identity.
    #[error("the image recipe has no SVG renderer identity")]
    NoSvgRendererIdentity,
    /// The source changed after its variants were prepared.
    #[error("the image changed since Tola prepared its variants; run the build again")]
    SourceChanged,
    /// JPEG would drop the source transparency.
    #[error(
        "JPEG output cannot keep transparency; set `background`, or use `format: \"png\"` or `format: \"webp\"`"
    )]
    JpegTransparency,
}

/// How a request's dimensions shape the derivative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeOperation {
    /// Exactly the requested size, changing the aspect ratio where it does not match the source's.
    Scale,
    /// The requested width, keeping the aspect ratio.
    FitWidth,
    /// The requested height, keeping the aspect ratio.
    FitHeight,
    /// Inside the requested box, keeping the aspect ratio, never enlarging the source.
    Fit,
    /// Exactly the requested size, cropped to the requested shape, or enlarged when the source is
    /// smaller than the request.
    Fill,
}

/// The part of the source `fill` keeps when the requested shape differs from the source's.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum CropAnchor {
    /// The center of each axis.
    #[default]
    Center,
    /// Centered horizontally, flush with the top.
    Top,
    /// Centered horizontally, flush with the bottom.
    Bottom,
    /// Flush with the left edge, centered vertically.
    Left,
    /// Flush with the right edge, centered vertically.
    Right,
    /// Flush with the top-left corner.
    TopLeft,
    /// Flush with the top-right corner.
    TopRight,
    /// Flush with the bottom-left corner.
    BottomLeft,
    /// Flush with the bottom-right corner.
    BottomRight,
}

impl CropAnchor {
    /// Every anchor, for validating a stored crop.
    const ALL: [Self; 9] = [
        Self::Center,
        Self::Top,
        Self::Bottom,
        Self::Left,
        Self::Right,
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// The horizontal and vertical placement this anchor asks for.
    pub(super) fn alignments(self) -> (Placement, Placement) {
        use CropAnchor::*;
        match self {
            Center => (Placement::Center, Placement::Center),
            Top => (Placement::Center, Placement::Start),
            Bottom => (Placement::Center, Placement::End),
            Left => (Placement::Start, Placement::Center),
            Right => (Placement::End, Placement::Center),
            TopLeft => (Placement::Start, Placement::Start),
            TopRight => (Placement::End, Placement::Start),
            BottomLeft => (Placement::Start, Placement::End),
            BottomRight => (Placement::End, Placement::End),
        }
    }
}

/// One axis of a crop anchor.
#[derive(Clone, Copy)]
pub(super) enum Placement {
    Start,
    Center,
    End,
}

impl Placement {
    /// Where the crop starts along an axis of length `span`, leaving `extent` for itself.
    pub(super) fn offset(self, span: f64, extent: f64) -> f64 {
        match self {
            Self::Start => 0.0,
            Self::Center => (span - extent) / 2.0,
            Self::End => span - extent,
        }
    }
}

/// What a request asks its derivative to be encoded as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    /// JPEG for a lossy source without transparency, PNG otherwise.
    Auto,
    /// JPEG at the requested quality, composited onto the background when the source has coverage.
    Jpeg,
    /// PNG, always lossless.
    Png,
    /// WebP: lossless without a quality, lossy with one.
    WebP,
}

/// One request, before it has been resolved against a source's metadata.
///
/// Defaults to centered fill, automatic format, and Lanczos3 without a quality or background.
/// Both dimensions must still be supplied before resolving the default request.
#[derive(Clone, Copy, Debug)]
pub struct ResizeOptions {
    /// The requested width, which every operation but `fit_height` needs.
    pub width: Option<u32>,
    /// The requested height, which every operation but `fit_width` needs.
    pub height: Option<u32>,
    /// How the requested dimensions shape the derivative.
    pub operation: ResizeOperation,
    /// Which part of the source `fill` keeps when the shapes differ.
    pub anchor: CropAnchor,
    /// The encoding the derivative is published as.
    pub format: OutputFormat,
    /// JPEG quality from 1 to 100, defaulting to 75, or WebP quality from 0 to 100; PNG ignores
    /// it.
    pub quality: Option<u8>,
    /// The kernel the resize convolves with.
    pub filter: ResizeFilter,
    /// Composite the result onto this opaque color instead of keeping transparency.
    pub background: Option<Background>,
}

impl Default for ResizeOptions {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            operation: ResizeOperation::Fill,
            anchor: CropAnchor::Center,
            format: OutputFormat::Auto,
            quality: None,
            filter: ResizeFilter::default(),
            background: None,
        }
    }
}

/// The resampling kernel one resize convolves with.
///
/// A kernel is evaluated into fixed-point weights before the convolution, so this choice changes
/// which source samples a resize keeps, never how its arithmetic is carried out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeFilter {
    /// Point sampling: the source pixel nearest an output pixel's center.
    Nearest,
    /// Antialiased triangle: the smoothest kernel here, and the cheapest.
    Triangle,
    /// Catmull-Rom cubic: sharper than the triangle, with mild ringing.
    CatmullRom,
    /// Gaussian: softens detail instead of restoring it.
    Gaussian,
    /// Lanczos with three lobes: the sharpest kernel here without visible aliasing.
    #[default]
    Lanczos3,
}

/// The opaque color a resized image is composited onto instead of keeping its transparency.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Background {
    /// Red channel, sRGB.
    pub red: u8,
    /// Green channel, sRGB.
    pub green: u8,
    /// Blue channel, sRGB.
    pub blue: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Crop {
    fn full(meta: &ImageMetadata) -> Self {
        Self {
            x: 0,
            y: 0,
            width: meta.width,
            height: meta.height,
        }
    }

    fn anchored(meta: &ImageMetadata, w: u32, h: u32, anchor: CropAnchor) -> Self {
        let (crop_w, crop_h) =
            if u64::from(meta.width) * u64::from(h) > u64::from(meta.height) * u64::from(w) {
                (
                    ((u64::from(meta.height) * u64::from(w)) / u64::from(h)).max(1) as u32,
                    meta.height,
                )
            } else {
                (
                    meta.width,
                    ((u64::from(meta.width) * u64::from(h)) / u64::from(w)).max(1) as u32,
                )
            };
        let (horiz, vert) = anchor.alignments();
        Self {
            x: horiz.offset(f64::from(meta.width), f64::from(crop_w)) as u32,
            y: vert.offset(f64::from(meta.height), f64::from(crop_h)) as u32,
            width: crop_w,
            height: crop_h,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Encoding {
    Jpeg {
        quality: u8,
    },
    Png,
    /// None is lossless, including the RGB values behind fully transparent pixels.
    WebP {
        quality: Option<u8>,
    },
}

/// The pixels one recipe asks for.
///
/// Recipes that share this value resample identical pixels, so one resample serves every encoding
/// of the same geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PixelRecipe {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) crop: Option<Crop>,
    pub(super) vector: Option<VectorGeometry>,
    pub(super) filter: ResizeFilter,
    pub(super) background: Option<Background>,
}

impl PixelRecipe {
    /// The width these pixels are rendered to.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// The height these pixels are rendered to.
    pub const fn height(self) -> u32 {
        self.height
    }

    /// The source rectangle a vector recipe renders, when this is a vector recipe.
    pub const fn vector_geometry(self) -> Option<VectorGeometry> {
        self.vector
    }

    pub(super) const fn raster_crop(self) -> Option<Crop> {
        self.crop
    }

    pub(super) fn normalize_filter(&mut self) {
        match (self.crop, self.vector) {
            // Copying an unchanged crop bypasses convolution, regardless of the requested filter.
            (Some(crop), _) if (self.width, self.height) == (crop.width, crop.height) => {
                self.filter = ResizeFilter::default();
            }
            // A vector is rendered at the target size; no kernel participates.
            (None, Some(_)) => self.filter = ResizeFilter::default(),
            _ => {}
        }
    }

    fn validate_structure(self) -> Result<(), ImageRequestError> {
        check_dimensions(self.width, self.height)
            .map_err(|_| ImageRequestError::InvalidDimensions)?;
        match (self.crop, self.vector) {
            (Some(crop), None) => {
                check_dimensions(crop.width, crop.height)
                    .map_err(|_| ImageRequestError::InvalidDimensions)?;
                if crop.x.checked_add(crop.width).is_none()
                    || crop.y.checked_add(crop.height).is_none()
                {
                    return Err(ImageRequestError::CropCoordinatesOverflow);
                }
            }
            (None, Some(vector)) => vector.validate()?,
            _ => return Err(ImageRequestError::SourceRegionCount),
        }
        Ok(())
    }

    /// Whether these pixels can be resampled from an image with this metadata.
    pub fn validate(self, meta: &ImageMetadata) -> Result<(), ImageRequestError> {
        let Some(crop) = self.raster_crop() else {
            return Err(ImageRequestError::VectorRecipeNotResampled);
        };
        check_dimensions(meta.width, meta.height)
            .map_err(|_| ImageRequestError::InvalidDimensions)?;
        if crop.x + crop.width > meta.width || crop.y + crop.height > meta.height {
            return Err(ImageRequestError::CropOutsideSource);
        }
        if crop != Crop::full(meta)
            && !CropAnchor::ALL
                .into_iter()
                .any(|anchor| crop == Crop::anchored(meta, self.width, self.height, anchor))
        {
            return Err(ImageRequestError::CropNotAnchoredFill);
        }
        Ok(())
    }
}

/// Only resolved, output-affecting values belong here. Equivalent operations and ignored options
/// therefore share a derivative; source identity and presentation never enter this value.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RecipeFields", into = "RecipeFields")]
pub struct ImageRecipe {
    pub(super) pixels: PixelRecipe,
    pub(super) encoding: Encoding,
    pub(super) svg: Option<SvgRender>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFields {
    width: u32,
    height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    crop: Option<Crop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vector: Option<VectorGeometry>,
    filter: ResizeFilter,
    background: Option<Background>,
    encoding: Encoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    svg: Option<SvgRender>,
}

impl From<ImageRecipe> for RecipeFields {
    fn from(recipe: ImageRecipe) -> Self {
        Self {
            width: recipe.pixels.width,
            height: recipe.pixels.height,
            crop: recipe.pixels.crop,
            vector: recipe.pixels.vector,
            filter: recipe.pixels.filter,
            background: recipe.pixels.background,
            encoding: recipe.encoding,
            svg: recipe.svg,
        }
    }
}

impl TryFrom<RecipeFields> for ImageRecipe {
    type Error = ImageRequestError;

    fn try_from(fields: RecipeFields) -> Result<Self, Self::Error> {
        let mut recipe = Self {
            pixels: PixelRecipe {
                width: fields.width,
                height: fields.height,
                crop: fields.crop,
                vector: fields.vector,
                filter: fields.filter,
                background: fields.background,
            },
            encoding: fields.encoding,
            svg: fields.svg,
        };
        recipe.pixels.normalize_filter();
        recipe.validate_structure()?;
        Ok(recipe)
    }
}

impl ImageRecipe {
    /// Resolve one request against a source's metadata, or refuse it with what the site author
    /// can change.
    ///
    /// The resolved recipe carries the encoding the request settled on, which is never
    /// [`OutputFormat::Auto`].
    pub fn resolve(meta: &ImageMetadata, opts: ResizeOptions) -> Result<Self, ImageRequestError> {
        check_dimensions(meta.width, meta.height)
            .map_err(|_| ImageRequestError::InvalidDimensions)?;
        validate_request(opts)?;
        let mut crop = Crop::full(meta);
        let (w, h) = match opts.operation {
            ResizeOperation::Scale => (requested_width(&opts)?, requested_height(&opts)?),
            ResizeOperation::FitWidth => {
                let w = requested_width(&opts)?;
                (w, scaled_dimension(meta.height, w, meta.width)?)
            }
            ResizeOperation::FitHeight => {
                let h = requested_height(&opts)?;
                (scaled_dimension(meta.width, h, meta.height)?, h)
            }
            ResizeOperation::Fit => {
                let w = requested_width(&opts)?;
                let h = requested_height(&opts)?;
                if meta.width <= w && meta.height <= h {
                    (meta.width, meta.height)
                } else if u64::from(w) * u64::from(meta.height)
                    <= u64::from(h) * u64::from(meta.width)
                {
                    (w, scaled_dimension(meta.height, w, meta.width)?.min(h))
                } else {
                    (scaled_dimension(meta.width, h, meta.height)?.min(w), h)
                }
            }
            ResizeOperation::Fill => {
                let w = requested_width(&opts)?;
                let h = requested_height(&opts)?;
                crop = Crop::anchored(meta, w, h, opts.anchor);
                (w, h)
            }
        };
        let format = select_format(opts.format, meta.is_lossy, meta.has_alpha);
        let encoding = encoding_for(format, opts.quality);
        let mut recipe = Self {
            pixels: PixelRecipe {
                width: w,
                height: h,
                crop: Some(crop),
                vector: None,
                filter: opts.filter,
                background: opts.background.filter(|_| meta.has_alpha),
            },
            encoding,
            svg: None,
        };
        recipe.pixels.normalize_filter();
        recipe.validate_structure()?;
        recipe.validate(meta)?;
        Ok(recipe)
    }

    /// The pixels this recipe asks for, which several encodings can share.
    pub const fn pixels(&self) -> PixelRecipe {
        self.pixels
    }

    /// The width of the derivative this publishes.
    pub const fn width(&self) -> u32 {
        self.pixels.width
    }

    /// The height of the derivative this publishes.
    pub const fn height(&self) -> u32 {
        self.pixels.height
    }

    /// The format the derivative is published as.
    pub const fn format(&self) -> ImageFormat {
        match self.encoding {
            Encoding::Jpeg { .. } => ImageFormat::Jpeg,
            Encoding::Png => ImageFormat::Png,
            Encoding::WebP { .. } => ImageFormat::WebP,
        }
    }

    /// The SVG renderer identity, when this recipe rasterizes an SVG.
    pub const fn svg_render(&self) -> Option<SvgRender> {
        self.svg
    }

    /// The source rectangle a vector recipe renders, when this is a vector recipe.
    pub const fn vector_geometry(&self) -> Option<VectorGeometry> {
        self.pixels.vector
    }

    pub(super) fn validate_structure(&self) -> Result<(), ImageRequestError> {
        self.pixels.validate_structure()?;
        match (self.pixels.crop, self.pixels.vector, self.svg) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => {}
            _ => return Err(ImageRequestError::MixedSourceGeometry),
        }
        if let Some(svg) = self.svg {
            svg.validate()?;
        }
        match self.encoding {
            Encoding::Jpeg { quality } => {
                if !(1..=100).contains(&quality) {
                    return Err(ImageRequestError::JpegQuality);
                }
                if self.width() > 65_535 || self.height() > 65_535 {
                    return Err(ImageRequestError::JpegDimensions);
                }
            }
            Encoding::WebP { quality } => {
                if !quality.is_none_or(|q| q <= 100) {
                    return Err(ImageRequestError::WebpQuality);
                }
                if self.width() > 16_383 || self.height() > 16_383 {
                    return Err(ImageRequestError::WebpDimensions);
                }
            }
            Encoding::Png => {}
        }
        Ok(())
    }

    /// Whether a source with this metadata can be rendered by this raster recipe.
    pub fn validate(&self, meta: &ImageMetadata) -> Result<(), ImageRequestError> {
        self.pixels.validate(meta)?;
        self.validate_colors(meta.has_alpha)
    }

    pub(super) fn validate_colors(&self, has_alpha: bool) -> Result<(), ImageRequestError> {
        if self.format() == ImageFormat::Jpeg && has_alpha && self.pixels.background.is_none() {
            return Err(ImageRequestError::JpegTransparency);
        }
        Ok(())
    }
}

/// The shape every request must have, whatever source it names.
pub(super) fn validate_request(opts: ResizeOptions) -> Result<(), ImageRequestError> {
    if opts.operation != ResizeOperation::Fill && opts.anchor != CropAnchor::Center {
        return Err(ImageRequestError::AnchorWithoutFill);
    }
    if [opts.width, opts.height]
        .into_iter()
        .flatten()
        .any(|dim| dim == 0)
    {
        return Err(ImageRequestError::NonPositiveDimensions);
    }
    Ok(())
}

/// The width a request must carry for its operation.
pub(super) fn requested_width(opts: &ResizeOptions) -> Result<u32, ImageRequestError> {
    opts.width
        .ok_or(ImageRequestError::MissingDimension { dimension: "width" })
}

/// The height a request must carry for its operation.
pub(super) fn requested_height(opts: &ResizeOptions) -> Result<u32, ImageRequestError> {
    opts.height.ok_or(ImageRequestError::MissingDimension {
        dimension: "height",
    })
}

/// Nearest integer, with ties upward, and at least one pixel for extremely thin images.
fn scaled_dimension(dim: u32, num: u32, den: u32) -> Result<u32, ImageRequestError> {
    let scaled = ((u64::from(dim) * u64::from(num) + u64::from(den) / 2) / u64::from(den)).max(1);
    u32::try_from(scaled).map_err(|_| ImageRequestError::ResizedTooLarge)
}

fn select_format(format: OutputFormat, is_lossy: bool, has_alpha: bool) -> OutputFormat {
    match format {
        OutputFormat::Auto if is_lossy && !has_alpha => OutputFormat::Jpeg,
        OutputFormat::Auto => OutputFormat::Png,
        format => format,
    }
}

pub(super) fn encoding_for(format: OutputFormat, quality: Option<u8>) -> Encoding {
    match format {
        OutputFormat::Jpeg => Encoding::Jpeg {
            quality: quality.unwrap_or(75),
        },
        OutputFormat::Png => Encoding::Png,
        OutputFormat::WebP => Encoding::WebP { quality },
        OutputFormat::Auto => unreachable!("auto encoding was resolved"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_oversized_scaled_dim() {
        assert!(scaled_dimension(u32::MAX, 2, 1).is_err());
        assert_eq!(scaled_dimension(u32::MAX, 1, 1).unwrap(), u32::MAX);
    }

    fn metadata(w: u32, h: u32) -> ImageMetadata {
        ImageMetadata {
            width: w,
            height: h,
            format: ImageFormat::Png,
            has_alpha: false,
            is_lossy: false,
        }
    }

    type GeometryCase = (ResizeOperation, Option<u32>, Option<u32>, (u32, u32));

    const GEOMETRY_CASES: [GeometryCase; 5] = [
        (ResizeOperation::Scale, Some(150), Some(150), (150, 150)),
        (ResizeOperation::FitWidth, Some(100), None, (100, 127)),
        (ResizeOperation::FitHeight, None, Some(150), (118, 150)),
        (ResizeOperation::Fit, Some(150), Some(150), (118, 150)),
        (ResizeOperation::Fill, Some(150), Some(150), (150, 150)),
    ];

    fn options(op: ResizeOperation, w: Option<u32>, h: Option<u32>) -> ResizeOptions {
        ResizeOptions {
            width: w,
            height: h,
            operation: op,
            anchor: CropAnchor::Center,
            format: OutputFormat::Auto,
            quality: None,
            filter: ResizeFilter::Lanczos3,
            background: None,
        }
    }

    #[test]
    fn default_resolves_fill() {
        for alpha in [false, true] {
            let src = ImageMetadata {
                format: ImageFormat::WebP,
                has_alpha: alpha,
                is_lossy: true,
                ..metadata(300, 200)
            };
            let req = ResizeOptions {
                width: Some(100),
                height: Some(100),
                ..ResizeOptions::default()
            };
            assert_eq!(
                ImageRecipe::resolve(&src, req).unwrap(),
                ImageRecipe::resolve(&src, options(ResizeOperation::Fill, Some(100), Some(100)))
                    .unwrap(),
            );
        }
    }

    #[test]
    fn fill_keeps_anchor() {
        let wide = metadata(100, 50);
        let tall = metadata(50, 100);
        for (anchor, wide_x, tall_y) in [
            (CropAnchor::Center, 25, 25),
            (CropAnchor::Top, 25, 0),
            (CropAnchor::Bottom, 25, 50),
            (CropAnchor::Left, 0, 25),
            (CropAnchor::Right, 50, 25),
            (CropAnchor::TopLeft, 0, 0),
            (CropAnchor::TopRight, 50, 0),
            (CropAnchor::BottomLeft, 0, 50),
            (CropAnchor::BottomRight, 50, 50),
        ] {
            let mut req = options(ResizeOperation::Fill, Some(50), Some(50));
            req.anchor = anchor;
            let crop = ImageRecipe::resolve(&wide, req)
                .unwrap()
                .pixels()
                .raster_crop()
                .unwrap();
            assert_eq!((crop.x, crop.y), (wide_x, 0), "wide {anchor:?}");
            let crop = ImageRecipe::resolve(&tall, req)
                .unwrap()
                .pixels()
                .raster_crop()
                .unwrap();
            assert_eq!((crop.x, crop.y), (0, tall_y), "tall {anchor:?}");
        }
    }

    #[test]
    fn reject_non_fill_anchor() {
        for op in [
            ResizeOperation::Scale,
            ResizeOperation::FitWidth,
            ResizeOperation::FitHeight,
            ResizeOperation::Fit,
        ] {
            let mut req = options(op, Some(50), Some(50));
            req.anchor = CropAnchor::Top;
            assert!(
                ImageRecipe::resolve(&metadata(100, 50), req).is_err(),
                "{op:?}"
            );
        }
    }

    #[test]
    fn fill_requires_dims() {
        for (w, h) in [(None, None), (Some(100), None), (None, Some(100))] {
            let req = ResizeOptions {
                width: w,
                height: h,
                ..ResizeOptions::default()
            };
            assert!(ImageRecipe::resolve(&metadata(300, 200), req).is_err());
        }
    }

    #[test]
    fn raster_display_geometry() {
        let src = metadata(300, 380);
        for (op, w, h, expected) in GEOMETRY_CASES {
            let recipe = ImageRecipe::resolve(&src, options(op, w, h)).unwrap();
            assert_eq!((recipe.width(), recipe.height()), expected);
        }
        let fill = ImageRecipe::resolve(&src, options(ResizeOperation::Fill, Some(150), Some(150)))
            .unwrap();
        assert_eq!(
            fill.pixels.raster_crop().unwrap(),
            Crop {
                x: 0,
                y: 40,
                width: 300,
                height: 300
            }
        );
    }

    #[test]
    fn fit_upscale_behavior() {
        let src = metadata(3, 2);
        let fit =
            ImageRecipe::resolve(&src, options(ResizeOperation::Fit, Some(20), Some(20))).unwrap();
        assert_eq!((fit.width(), fit.height()), (3, 2));
        let wide =
            ImageRecipe::resolve(&src, options(ResizeOperation::FitWidth, Some(12), None)).unwrap();
        assert_eq!((wide.width(), wide.height()), (12, 8));
        let thin = ImageRecipe::resolve(
            &metadata(16_384, 1),
            options(ResizeOperation::FitWidth, Some(1), None),
        )
        .unwrap();
        assert_eq!((thin.width(), thin.height()), (1, 1));
    }

    #[test]
    fn jpeg_dim_limit() {
        let src = ImageMetadata {
            format: ImageFormat::Jpeg,
            is_lossy: true,
            ..metadata(1, 1)
        };
        for fmt in [OutputFormat::Jpeg, OutputFormat::Auto] {
            for (w, h) in [(65_535, 1), (1, 65_535)] {
                let mut req = options(ResizeOperation::Scale, Some(w), Some(h));
                req.format = fmt;
                let recipe = ImageRecipe::resolve(&src, req).unwrap();
                assert_eq!((recipe.width(), recipe.height()), (w, h));
                assert_eq!(recipe.format(), ImageFormat::Jpeg);
            }
            for (w, h) in [(65_536, 1), (1, 65_536)] {
                let mut req = options(ResizeOperation::Scale, Some(w), Some(h));
                req.format = fmt;
                assert!(ImageRecipe::resolve(&src, req).is_err());
            }
        }
    }

    #[test]
    fn jpeg_limit_uses_resolved_dims() {
        let src = metadata(3, 2);
        let mut req = options(ResizeOperation::Fit, Some(65_536), Some(65_536));
        req.format = OutputFormat::Jpeg;
        let recipe = ImageRecipe::resolve(&src, req).unwrap();
        assert_eq!((recipe.width(), recipe.height()), (3, 2));
    }

    #[test]
    fn equivalent_recipes_dedup() {
        let src = metadata(300, 200);
        let lanczos =
            ImageRecipe::resolve(&src, options(ResizeOperation::FitWidth, Some(150), None))
                .unwrap();
        let mut scaled = options(ResizeOperation::Scale, Some(150), Some(100));
        scaled.quality = Some(91);
        scaled.format = OutputFormat::Png;
        assert_eq!(lanczos, ImageRecipe::resolve(&src, scaled).unwrap());
        let mut tri = options(ResizeOperation::FitWidth, Some(150), None);
        tri.filter = ResizeFilter::Triangle;
        assert_ne!(lanczos, ImageRecipe::resolve(&src, tri).unwrap());
        tri.filter = ResizeFilter::Lanczos3;
        assert_eq!(lanczos, ImageRecipe::resolve(&src, tri).unwrap());
    }

    #[test]
    fn unused_filter_ignored() {
        for (op, w, h) in [
            (ResizeOperation::Fit, 12, 12),
            (ResizeOperation::Fill, 6, 6),
        ] {
            let mut req = options(op, Some(w), Some(h));
            let recipe = ImageRecipe::resolve(&metadata(9, 6), req).unwrap();
            req.filter = ResizeFilter::Triangle;
            assert_eq!(recipe, ImageRecipe::resolve(&metadata(9, 6), req).unwrap());
        }
    }

    #[test]
    fn serde_ignores_unused_filter() {
        let src = metadata(9, 6);
        let recipe =
            ImageRecipe::resolve(&src, options(ResizeOperation::Fill, Some(6), Some(6))).unwrap();
        let mut val = serde_json::to_value(&recipe).unwrap();
        val["filter"] = "triangle".into();
        assert_eq!(recipe, serde_json::from_value::<ImageRecipe>(val).unwrap());
    }

    #[test]
    fn background_follows_alpha() {
        for alpha in [false, true] {
            let src = ImageMetadata {
                has_alpha: alpha,
                ..metadata(300, 200)
            };
            let mut req = options(ResizeOperation::FitWidth, Some(150), None);
            let recipe = ImageRecipe::resolve(&src, req).unwrap();
            req.background = Some(Background {
                red: 0,
                green: 0,
                blue: 0,
            });
            let flat = ImageRecipe::resolve(&src, req).unwrap();
            if alpha {
                assert_ne!(recipe, flat);
            } else {
                assert_eq!(recipe, flat);
            }
        }
    }

    #[test]
    fn fill_crop_target_ratio() {
        let src = metadata(9, 6);
        let recipe =
            ImageRecipe::resolve(&src, options(ResizeOperation::Fill, Some(4), Some(4))).unwrap();
        assert_eq!(
            recipe.pixels.raster_crop().unwrap(),
            Crop {
                x: 1,
                y: 0,
                width: 6,
                height: 6
            }
        );
        assert!(recipe.validate(&metadata(4, 4)).is_err());
        let mut forged = recipe;
        forged.pixels.crop.as_mut().unwrap().x = 2;
        assert!(forged.validate(&src).is_err());
    }

    #[test]
    fn serde_rejects_invalid_bounds() {
        for json in [
            r#"{"width":0,"height":1,"crop":{"x":0,"y":0,"width":1,"height":1},"filter":"lanczos3","background":null,"encoding":"png"}"#,
            r#"{"width":1,"height":1,"crop":{"x":4294967295,"y":0,"width":1,"height":1},"filter":"triangle","background":null,"encoding":"png"}"#,
            r#"{"width":1,"height":1,"crop":{"x":0,"y":0,"width":1,"height":1},"filter":"catmull_rom","background":null,"encoding":{"jpeg":{"quality":0}}}"#,
            r#"{"width":1,"height":65536,"crop":{"x":0,"y":0,"width":1,"height":1},"filter":"lanczos3","background":null,"encoding":{"jpeg":{"quality":75}}}"#,
            r#"{"width":1,"height":1,"crop":{"x":0,"y":0,"width":1,"height":1},"filter":"gaussian","background":null,"encoding":{"web_p":{"quality":101}}}"#,
            r#"{"width":1,"height":1,"crop":{"x":0,"y":0,"width":1,"height":1},"filter":"nearest","background":{"red":0,"green":0},"encoding":"png"}"#,
        ] {
            assert!(serde_json::from_str::<ImageRecipe>(json).is_err());
        }
    }

    #[test]
    fn alpha_forces_png_or_bg() {
        let src = ImageMetadata {
            format: ImageFormat::WebP,
            has_alpha: true,
            is_lossy: true,
            ..metadata(1, 1)
        };
        let mut args = options(ResizeOperation::Scale, Some(1), Some(1));
        assert_eq!(
            ImageRecipe::resolve(&src, args).unwrap().format(),
            ImageFormat::Png
        );
        args.format = OutputFormat::Jpeg;
        assert!(ImageRecipe::resolve(&src, args).is_err());
        args.background = Some(Background {
            red: 255,
            green: 255,
            blue: 255,
        });
        assert_eq!(
            ImageRecipe::resolve(&src, args).unwrap().format(),
            ImageFormat::Jpeg
        );
    }

    #[test]
    fn auto_encoding_follows_src() {
        let lossy = ImageMetadata {
            is_lossy: true,
            ..metadata(40, 30)
        };
        let resolved =
            ImageRecipe::resolve(&lossy, options(ResizeOperation::Scale, Some(20), Some(20)))
                .expect("a valid recipe");
        assert_eq!(resolved.encoding, Encoding::Jpeg { quality: 75 });

        let opaque = metadata(40, 30);
        let resolved =
            ImageRecipe::resolve(&opaque, options(ResizeOperation::Scale, Some(20), Some(20)))
                .expect("a valid recipe");
        assert_eq!(resolved.encoding, Encoding::Png);
    }

    #[test]
    fn serde_roundtrip_geometry() {
        let vec_opts = ResizeOptions {
            format: OutputFormat::Png,
            ..options(ResizeOperation::Scale, Some(5), Some(5))
        };
        for (recipe, present, absent) in [
            (
                ImageRecipe::resolve(
                    &metadata(10, 10),
                    options(ResizeOperation::Scale, Some(5), Some(5)),
                )
                .unwrap(),
                &["crop"][..],
                &["vector", "svg"][..],
            ),
            (
                ImageRecipe::resolve_svg((10.0, 10.0), vec_opts, SvgRender::new(9)).unwrap(),
                &["vector", "svg"][..],
                &["crop"][..],
            ),
        ] {
            let val = serde_json::to_value(&recipe).unwrap();
            for field in present {
                assert!(val.get(field).is_some(), "{field}: {val}");
            }
            for field in absent {
                assert!(val.get(field).is_none(), "{field}: {val}");
            }
            assert_eq!(serde_json::from_value::<ImageRecipe>(val).unwrap(), recipe);
        }
    }
}
