use std::hash::{Hash, Hasher};

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::{ImageFormat, ImageMetadata, check_dimensions};

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
    fn alignments(self) -> (Placement, Placement) {
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
enum Placement {
    Start,
    Center,
    End,
}

impl Placement {
    /// Where the crop starts along an axis of length `span`, leaving `extent` for itself.
    fn offset(self, span: f64, extent: f64) -> f64 {
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
    fn full(metadata: &ImageMetadata) -> Self {
        Self {
            x: 0,
            y: 0,
            width: metadata.width,
            height: metadata.height,
        }
    }

    fn anchored(metadata: &ImageMetadata, width: u32, height: u32, anchor: CropAnchor) -> Self {
        let (crop_width, crop_height) = if u64::from(metadata.width) * u64::from(height)
            > u64::from(metadata.height) * u64::from(width)
        {
            (
                ((u64::from(metadata.height) * u64::from(width)) / u64::from(height)).max(1) as u32,
                metadata.height,
            )
        } else {
            (
                metadata.width,
                ((u64::from(metadata.width) * u64::from(height)) / u64::from(width)).max(1) as u32,
            )
        };
        let (horizontal, vertical) = anchor.alignments();
        Self {
            x: horizontal.offset(f64::from(metadata.width), f64::from(crop_width)) as u32,
            y: vertical.offset(f64::from(metadata.height), f64::from(crop_height)) as u32,
            width: crop_width,
            height: crop_height,
        }
    }
}

/// One exact coordinate or extent, compared by its bits so recipes stay `Eq` and `Hash`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(into = "u64", try_from = "u64")]
struct Exact(f64);

impl Exact {
    fn checked(value: f64) -> Result<Self> {
        ensure!(value.is_finite(), "the SVG source geometry is not finite");
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }

    const fn get(self) -> f64 {
        self.0
    }
}

impl From<Exact> for u64 {
    fn from(value: Exact) -> Self {
        value.0.to_bits()
    }
}

impl TryFrom<u64> for Exact {
    type Error = anyhow::Error;

    fn try_from(bits: u64) -> Result<Self> {
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
    fn new(source: (f64, f64), crop: (f64, f64, f64, f64)) -> Result<Self> {
        let geometry = Self {
            source: (Exact::checked(source.0)?, Exact::checked(source.1)?),
            crop: (
                Exact::checked(crop.0)?,
                Exact::checked(crop.1)?,
                Exact::checked(crop.2)?,
                Exact::checked(crop.3)?,
            ),
        };
        geometry.validate()?;
        Ok(geometry)
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

    fn validate(self) -> Result<()> {
        let (source_width, source_height) = self.source();
        let (x, y, width, height) = self.crop();
        ensure!(
            source_width > 0.0
                && source_height > 0.0
                && x >= 0.0
                && y >= 0.0
                && width > 0.0
                && height > 0.0,
            "the SVG source geometry is invalid"
        );
        let slack_x = 1e-9 * source_width.max(1.0);
        let slack_y = 1e-9 * source_height.max(1.0);
        ensure!(
            x + width <= source_width + slack_x && y + height <= source_height + slack_y,
            "the SVG crop is outside the source"
        );
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

    fn validate(self) -> Result<()> {
        ensure!(
            self.revision == SVG_RENDERER_REVISION,
            "the SVG renderer revision is not supported; run the build again"
        );
        Ok(())
    }
}

/// Bump when the SVG renderer stack changes pixels for the same source and recipe.
pub const SVG_RENDERER_REVISION: u16 = 1;

mod hash_hex {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 32
            || !text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(D::Error::custom(
                "the SVG renderer hash must be 32 lowercase hexadecimal digits",
            ));
        }
        u128::from_str_radix(&text, 16).map_err(D::Error::custom)
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

    fn normalize_filter(&mut self) {
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

    fn validate_structure(self) -> Result<()> {
        check_dimensions(self.width, self.height)?;
        match (self.crop, self.vector) {
            (Some(crop), None) => {
                check_dimensions(crop.width, crop.height)?;
                ensure!(
                    crop.x.checked_add(crop.width).is_some()
                        && crop.y.checked_add(crop.height).is_some(),
                    "image crop coordinates overflow"
                );
            }
            (None, Some(vector)) => vector.validate()?,
            _ => bail!("the image recipe needs exactly one source region"),
        }
        Ok(())
    }

    /// Whether these pixels can be resampled from an image with this metadata.
    pub fn validate(self, metadata: &ImageMetadata) -> Result<()> {
        self.validate_structure()?;
        let Some(crop) = self.raster_crop() else {
            bail!("vector recipes are rendered, not resampled");
        };
        check_dimensions(metadata.width, metadata.height)?;
        ensure!(
            crop.x + crop.width <= metadata.width && crop.y + crop.height <= metadata.height,
            "the resize crop is outside the source image"
        );
        ensure!(
            crop == Crop::full(metadata)
                || CropAnchor::ALL
                    .into_iter()
                    .any(|anchor| crop == Crop::anchored(metadata, self.width, self.height, anchor)),
            "image crop must be the complete source or one of its anchored fill crops"
        );
        Ok(())
    }
}

/// Only resolved, output-affecting values belong here. Equivalent operations and ignored options
/// therefore share a derivative; source identity and presentation never enter this value.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RecipeFields", into = "RecipeFields")]
pub struct ImageRecipe {
    pixels: PixelRecipe,
    pub(super) encoding: Encoding,
    svg: Option<SvgRender>,
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
    type Error = anyhow::Error;

    fn try_from(fields: RecipeFields) -> Result<Self> {
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
    pub fn resolve(metadata: &ImageMetadata, options: ResizeOptions) -> Result<Self> {
        check_dimensions(metadata.width, metadata.height)?;
        validate_request(options)?;
        let mut crop = Crop::full(metadata);
        let (width, height) = match options.operation {
            ResizeOperation::Scale => (requested_width(&options)?, requested_height(&options)?),
            ResizeOperation::FitWidth => {
                let width = requested_width(&options)?;
                (
                    width,
                    scaled_dimension(metadata.height, width, metadata.width)?,
                )
            }
            ResizeOperation::FitHeight => {
                let height = requested_height(&options)?;
                (
                    scaled_dimension(metadata.width, height, metadata.height)?,
                    height,
                )
            }
            ResizeOperation::Fit => {
                let width = requested_width(&options)?;
                let height = requested_height(&options)?;
                if metadata.width <= width && metadata.height <= height {
                    (metadata.width, metadata.height)
                } else if u64::from(width) * u64::from(metadata.height)
                    <= u64::from(height) * u64::from(metadata.width)
                {
                    (
                        width,
                        scaled_dimension(metadata.height, width, metadata.width)?.min(height),
                    )
                } else {
                    (
                        scaled_dimension(metadata.width, height, metadata.height)?.min(width),
                        height,
                    )
                }
            }
            ResizeOperation::Fill => {
                let width = requested_width(&options)?;
                let height = requested_height(&options)?;
                crop = Crop::anchored(metadata, width, height, options.anchor);
                (width, height)
            }
        };
        let format = resolved_format(options.format, metadata.is_lossy, metadata.has_alpha);
        let encoding = encoding_for(format, options.quality);
        let mut recipe = Self {
            pixels: PixelRecipe {
                width,
                height,
                crop: Some(crop),
                vector: None,
                filter: options.filter,
                background: options.background.filter(|_| metadata.has_alpha),
            },
            encoding,
            svg: None,
        };
        recipe.pixels.normalize_filter();
        recipe.validate(metadata)?;
        Ok(recipe)
    }

    /// Resolve one SVG request against the source's exact pixel size and the renderer identity.
    ///
    /// The format must be explicit: an SVG has no source pixel format to infer from, and a raster
    /// derivative is never the silent default.
    pub fn resolve_svg(
        source: (f64, f64),
        options: ResizeOptions,
        render: SvgRender,
    ) -> Result<Self> {
        ensure!(
            source.0.is_finite() && source.1.is_finite() && source.0 > 0.0 && source.1 > 0.0,
            "the SVG has no valid intrinsic size"
        );
        ensure!(
            options.format != OutputFormat::Auto,
            "`resize-image` cannot choose a format for an SVG; pass `format: \"png\"`, `\"jpg\"` (or `\"jpeg\"`), or `\"webp\"` to rasterize it, or use `image()` for vector display"
        );
        validate_request(options)?;
        let mut crop = (0.0, 0.0, source.0, source.1);
        let (width, height) = match options.operation {
            ResizeOperation::Scale => (requested_width(&options)?, requested_height(&options)?),
            ResizeOperation::FitWidth => {
                let width = requested_width(&options)?;
                (
                    width,
                    scaled_pixels_f64(source.1, f64::from(width), source.0)?,
                )
            }
            ResizeOperation::FitHeight => {
                let height = requested_height(&options)?;
                (
                    scaled_pixels_f64(source.0, f64::from(height), source.1)?,
                    height,
                )
            }
            ResizeOperation::Fit => {
                let width = requested_width(&options)?;
                let height = requested_height(&options)?;
                if source.0 <= f64::from(width) && source.1 <= f64::from(height) {
                    (rounded_pixel(source.0)?, rounded_pixel(source.1)?)
                } else if f64::from(width) * source.1 <= f64::from(height) * source.0 {
                    (
                        width,
                        scaled_pixels_f64(source.1, f64::from(width), source.0)?.min(height),
                    )
                } else {
                    (
                        scaled_pixels_f64(source.0, f64::from(height), source.1)?.min(width),
                        height,
                    )
                }
            }
            ResizeOperation::Fill => {
                let width = requested_width(&options)?;
                let height = requested_height(&options)?;
                let scale = (f64::from(width) / source.0).max(f64::from(height) / source.1);
                let crop_width = f64::from(width) / scale;
                let crop_height = f64::from(height) / scale;
                let (horizontal, vertical) = options.anchor.alignments();
                crop = (
                    horizontal.offset(source.0, crop_width),
                    vertical.offset(source.1, crop_height),
                    crop_width,
                    crop_height,
                );
                (width, height)
            }
        };
        let geometry = VectorGeometry::new(source, crop)?;
        let encoding = encoding_for(options.format, options.quality);
        let mut recipe = Self {
            pixels: PixelRecipe {
                width,
                height,
                crop: None,
                vector: Some(geometry),
                filter: options.filter,
                background: options.background,
            },
            encoding,
            svg: Some(render),
        };
        recipe.pixels.normalize_filter();
        recipe.validate_structure()?;
        recipe.validate_colors(true)?;
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

    fn validate_structure(&self) -> Result<()> {
        self.pixels.validate_structure()?;
        match (self.pixels.crop, self.pixels.vector, self.svg) {
            (Some(_), None, None) | (None, Some(_), Some(_)) => {}
            _ => bail!("the image recipe mixes raster and vector source geometry"),
        }
        if let Some(svg) = self.svg {
            svg.validate()?;
        }
        match self.encoding {
            Encoding::Jpeg { quality } => {
                ensure!(
                    (1..=100).contains(&quality),
                    "`quality` must be 1 to 100 for JPEG output"
                );
                ensure!(
                    self.width() <= 65_535 && self.height() <= 65_535,
                    "JPEG output cannot exceed 65535 pixels wide or tall; reduce `width` or `height`"
                );
            }
            Encoding::WebP { quality } => {
                ensure!(
                    quality.is_none_or(|quality| quality <= 100),
                    "`quality` must be 0 to 100 for WebP output"
                );
                ensure!(
                    self.width() <= 16_383 && self.height() <= 16_383,
                    "WebP output cannot exceed 16383 pixels wide or tall; reduce `width` or `height`"
                );
            }
            Encoding::Png => {}
        }
        Ok(())
    }

    /// Whether a source with this metadata can be rendered by this raster recipe.
    pub fn validate(&self, metadata: &ImageMetadata) -> Result<()> {
        self.validate_structure()?;
        self.pixels.validate(metadata)?;
        self.validate_colors(metadata.has_alpha)
    }

    /// Whether the parsed SVG with this size and hash can be rendered by this recipe.
    pub fn validate_svg(&self, width: f64, height: f64, hash: u128) -> Result<()> {
        self.validate_structure()?;
        let Some(vector) = self.pixels.vector else {
            bail!("the image recipe is not a vector recipe");
        };
        let Some(svg) = self.svg else {
            bail!("the image recipe has no SVG renderer identity");
        };
        ensure!(
            svg.hash() == hash,
            "the image changed since Tola prepared its variants; run the build again"
        );
        let (source_width, source_height) = vector.source();
        let tolerance = 1e-6 * width.abs().max(1.0);
        ensure!(
            (source_width - width).abs() <= tolerance
                && (source_height - height).abs() <= tolerance,
            "the image changed since Tola prepared its variants; run the build again"
        );
        self.validate_colors(true)
    }

    fn validate_colors(&self, has_alpha: bool) -> Result<()> {
        ensure!(
            self.format() != ImageFormat::Jpeg || !has_alpha || self.pixels.background.is_some(),
            "JPEG output cannot keep transparency; set `background`, or use `format: \"png\"` or `format: \"webp\"`"
        );
        Ok(())
    }
}

/// The shape every request must have, whatever source it names.
fn validate_request(options: ResizeOptions) -> Result<()> {
    ensure!(
        options.operation == ResizeOperation::Fill || options.anchor == CropAnchor::Center,
        "`anchor` applies only to `op: \"fill\"`; the other operations keep the whole source"
    );
    for dimension in [options.width, options.height].into_iter().flatten() {
        ensure!(
            dimension > 0,
            "`width` and `height` must be positive pixel counts"
        );
    }
    Ok(())
}

/// The width a request must carry for its operation.
fn requested_width(options: &ResizeOptions) -> Result<u32> {
    options
        .width
        .ok_or_else(|| anyhow::anyhow!("`resize-image` needs `width` for this `op`; pass `width`"))
}

/// The height a request must carry for its operation.
fn requested_height(options: &ResizeOptions) -> Result<u32> {
    options.height.ok_or_else(|| {
        anyhow::anyhow!("`resize-image` needs `height` for this `op`; pass `height`")
    })
}

/// Nearest integer, with ties upward, and at least one pixel for extremely thin images.
fn scaled_dimension(dimension: u32, numerator: u32, denominator: u32) -> Result<u32> {
    let scaled = ((u64::from(dimension) * u64::from(numerator) + u64::from(denominator) / 2)
        / u64::from(denominator))
    .max(1);
    u32::try_from(scaled).map_err(|_| {
        anyhow::anyhow!("the resized image would be too large; use a smaller `width` and `height`")
    })
}

/// Nearest integer at least one pixel, refusing values that cannot be a pixel count.
fn rounded_pixel(value: f64) -> Result<u32> {
    let rounded = value.round().max(1.0);
    ensure!(
        rounded <= f64::from(u32::MAX),
        "the resized image would be too large; use a smaller `width` and `height`"
    );
    Ok(rounded as u32)
}

fn scaled_pixels_f64(dimension: f64, numerator: f64, denominator: f64) -> Result<u32> {
    rounded_pixel(dimension * numerator / denominator)
}

fn resolved_format(format: OutputFormat, is_lossy: bool, has_alpha: bool) -> OutputFormat {
    match format {
        OutputFormat::Auto if is_lossy && !has_alpha => OutputFormat::Jpeg,
        OutputFormat::Auto => OutputFormat::Png,
        format => format,
    }
}

fn encoding_for(format: OutputFormat, quality: Option<u8>) -> Encoding {
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
    fn oversized_scaled_dimension_is_rejected() {
        assert!(scaled_dimension(u32::MAX, 2, 1).is_err());
        assert_eq!(scaled_dimension(u32::MAX, 1, 1).unwrap(), u32::MAX);
    }
    fn metadata(width: u32, height: u32) -> ImageMetadata {
        ImageMetadata {
            width,
            height,
            format: ImageFormat::Png,
            has_alpha: false,
            is_lossy: false,
        }
    }

    /// One operation's request and the display geometry it resolves.
    type GeometryCase = (ResizeOperation, Option<u32>, Option<u32>, (u32, u32));

    /// The display geometry every operation resolves for the 300 × 380 test sources.
    const GEOMETRY_CASES: [GeometryCase; 5] = [
        (ResizeOperation::Scale, Some(150), Some(150), (150, 150)),
        (ResizeOperation::FitWidth, Some(100), None, (100, 127)),
        (ResizeOperation::FitHeight, None, Some(150), (118, 150)),
        (ResizeOperation::Fit, Some(150), Some(150), (118, 150)),
        (ResizeOperation::Fill, Some(150), Some(150), (150, 150)),
    ];

    fn options(
        operation: ResizeOperation,
        width: Option<u32>,
        height: Option<u32>,
    ) -> ResizeOptions {
        ResizeOptions {
            width,
            height,
            operation,
            anchor: CropAnchor::Center,
            format: OutputFormat::Auto,
            quality: None,
            filter: ResizeFilter::Lanczos3,
            background: None,
        }
    }

    #[test]
    fn default_requests_resolve_fill() {
        for has_alpha in [false, true] {
            let source = ImageMetadata {
                format: ImageFormat::WebP,
                has_alpha,
                is_lossy: true,
                ..metadata(300, 200)
            };
            let request = ResizeOptions {
                width: Some(100),
                height: Some(100),
                ..ResizeOptions::default()
            };
            assert_eq!(
                ImageRecipe::resolve(&source, request).unwrap(),
                ImageRecipe::resolve(
                    &source,
                    options(ResizeOperation::Fill, Some(100), Some(100)),
                )
                .unwrap(),
            );
        }
    }

    /// `fill` keeps the corner or edge its anchor names; the other axis stays centered.
    #[test]
    fn fill_keeps_the_anchored_part() {
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
            let mut request = options(ResizeOperation::Fill, Some(50), Some(50));
            request.anchor = anchor;
            let crop = ImageRecipe::resolve(&wide, request)
                .unwrap()
                .pixels()
                .raster_crop()
                .unwrap();
            assert_eq!((crop.x, crop.y), (wide_x, 0), "wide {anchor:?}");
            let crop = ImageRecipe::resolve(&tall, request)
                .unwrap()
                .pixels()
                .raster_crop()
                .unwrap();
            assert_eq!((crop.x, crop.y), (0, tall_y), "tall {anchor:?}");
        }
    }

    /// Only `fill` crops, so every other operation refuses an anchor that asks for one.
    #[test]
    fn anchor_is_refused_without_fill() {
        for operation in [
            ResizeOperation::Scale,
            ResizeOperation::FitWidth,
            ResizeOperation::FitHeight,
            ResizeOperation::Fit,
        ] {
            let mut request = options(operation, Some(50), Some(50));
            request.anchor = CropAnchor::Top;
            assert!(
                ImageRecipe::resolve(&metadata(100, 50), request).is_err(),
                "{operation:?}"
            );
        }
    }

    #[test]
    fn svg_fill_keeps_the_anchored_part() {
        let mut request = vector_options(ResizeOperation::Fill, Some(50), Some(50));
        request.anchor = CropAnchor::BottomRight;
        let recipe = ImageRecipe::resolve_svg((100.0, 50.0), request, SvgRender::new(9)).unwrap();
        let (x, y, width, height) = recipe.vector_geometry().unwrap().crop();
        assert_eq!((x, y), (50.0, 0.0));
        assert_eq!((width, height), (50.0, 50.0));

        let mut request = vector_options(ResizeOperation::Fit, Some(50), Some(50));
        request.anchor = CropAnchor::BottomRight;
        assert!(ImageRecipe::resolve_svg((100.0, 50.0), request, SvgRender::new(9)).is_err());
    }

    #[test]
    fn default_fill_requires_dimensions() {
        for (width, height) in [(None, None), (Some(100), None), (None, Some(100))] {
            let request = ResizeOptions {
                width,
                height,
                ..ResizeOptions::default()
            };
            assert!(ImageRecipe::resolve(&metadata(300, 200), request).is_err());
        }
    }

    #[test]
    fn operations_resolve_display_geometry() {
        let source = metadata(300, 380);
        for (operation, width, height, expected) in GEOMETRY_CASES {
            let recipe = ImageRecipe::resolve(&source, options(operation, width, height)).unwrap();
            assert_eq!((recipe.width(), recipe.height()), expected);
        }
        let fill = ImageRecipe::resolve(
            &source,
            options(ResizeOperation::Fill, Some(150), Some(150)),
        )
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
    fn fit_operations_differ_on_upscaling() {
        let source = metadata(3, 2);
        let fit = ImageRecipe::resolve(&source, options(ResizeOperation::Fit, Some(20), Some(20)))
            .unwrap();
        assert_eq!((fit.width(), fit.height()), (3, 2));
        let wide =
            ImageRecipe::resolve(&source, options(ResizeOperation::FitWidth, Some(12), None))
                .unwrap();
        assert_eq!((wide.width(), wide.height()), (12, 8));
        let thin = ImageRecipe::resolve(
            &metadata(16_384, 1),
            options(ResizeOperation::FitWidth, Some(1), None),
        )
        .unwrap();
        assert_eq!((thin.width(), thin.height()), (1, 1));
    }

    #[test]
    fn jpeg_dimensions_stop_at_limit() {
        let source = ImageMetadata {
            format: ImageFormat::Jpeg,
            is_lossy: true,
            ..metadata(1, 1)
        };
        for format in [OutputFormat::Jpeg, OutputFormat::Auto] {
            for (width, height) in [(65_535, 1), (1, 65_535)] {
                let mut request = options(ResizeOperation::Scale, Some(width), Some(height));
                request.format = format;
                let recipe = ImageRecipe::resolve(&source, request).unwrap();
                assert_eq!((recipe.width(), recipe.height()), (width, height));
                assert_eq!(recipe.format(), ImageFormat::Jpeg);
            }
            for (width, height) in [(65_536, 1), (1, 65_536)] {
                let mut request = options(ResizeOperation::Scale, Some(width), Some(height));
                request.format = format;
                assert!(ImageRecipe::resolve(&source, request).is_err());
            }
        }
    }

    #[test]
    fn jpeg_limit_uses_resolved_dimensions() {
        let source = metadata(3, 2);
        let mut request = options(ResizeOperation::Fit, Some(65_536), Some(65_536));
        request.format = OutputFormat::Jpeg;
        let recipe = ImageRecipe::resolve(&source, request).unwrap();
        assert_eq!((recipe.width(), recipe.height()), (3, 2));
    }

    #[test]
    fn equivalent_requests_share_one_recipe() {
        let source = metadata(300, 200);
        let lanczos =
            ImageRecipe::resolve(&source, options(ResizeOperation::FitWidth, Some(150), None))
                .unwrap();
        let mut scaled = options(ResizeOperation::Scale, Some(150), Some(100));
        scaled.quality = Some(91);
        scaled.format = OutputFormat::Png;
        assert_eq!(lanczos, ImageRecipe::resolve(&source, scaled).unwrap());
        let mut triangle = options(ResizeOperation::FitWidth, Some(150), None);
        triangle.filter = ResizeFilter::Triangle;
        assert_ne!(lanczos, ImageRecipe::resolve(&source, triangle).unwrap());
        triangle.filter = ResizeFilter::Lanczos3;
        assert_eq!(lanczos, ImageRecipe::resolve(&source, triangle).unwrap());
    }

    /// A recipe whose resize reads no filter answers the same whichever one the caller names.
    #[test]
    fn unused_filters_leave_recipes_unchanged() {
        for (operation, width, height) in [
            (ResizeOperation::Fit, 12, 12),
            (ResizeOperation::Fill, 6, 6),
        ] {
            let mut request = options(operation, Some(width), Some(height));
            let recipe = ImageRecipe::resolve(&metadata(9, 6), request).unwrap();
            request.filter = ResizeFilter::Triangle;
            assert_eq!(
                recipe,
                ImageRecipe::resolve(&metadata(9, 6), request).unwrap()
            );
        }

        let render = SvgRender::new(1);
        let mut triangle = vector_options(ResizeOperation::FitWidth, Some(100), None);
        triangle.filter = ResizeFilter::Triangle;
        let lanczos = ImageRecipe::resolve_svg(
            (300.0, 380.0),
            vector_options(ResizeOperation::FitWidth, Some(100), None),
            render,
        )
        .unwrap();
        assert_eq!(
            lanczos,
            ImageRecipe::resolve_svg((300.0, 380.0), triangle, render).unwrap()
        );
    }

    #[test]
    fn wire_recipes_ignore_unused_filters() {
        let source = metadata(9, 6);
        let recipe =
            ImageRecipe::resolve(&source, options(ResizeOperation::Fill, Some(6), Some(6)))
                .unwrap();
        let mut serialized = serde_json::to_value(&recipe).unwrap();
        serialized["filter"] = "triangle".into();
        assert_eq!(
            recipe,
            serde_json::from_value::<ImageRecipe>(serialized).unwrap()
        );
    }

    #[test]
    fn background_identity_follows_alpha() {
        for has_alpha in [false, true] {
            let source = ImageMetadata {
                has_alpha,
                ..metadata(300, 200)
            };
            let mut request = options(ResizeOperation::FitWidth, Some(150), None);
            let recipe = ImageRecipe::resolve(&source, request).unwrap();
            request.background = Some(Background {
                red: 0,
                green: 0,
                blue: 0,
            });
            let flattened = ImageRecipe::resolve(&source, request).unwrap();
            if has_alpha {
                assert_ne!(recipe, flattened);
            } else {
                assert_eq!(recipe, flattened);
            }
        }
    }

    #[test]
    fn fill_crop_keeps_the_target_ratio() {
        let source = metadata(9, 6);
        let recipe =
            ImageRecipe::resolve(&source, options(ResizeOperation::Fill, Some(4), Some(4)))
                .unwrap();
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
        assert!(forged.validate(&source).is_err());
    }

    #[test]
    fn wire_recipes_reject_invalid_bounds() {
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
    fn transparency_forces_png_or_background() {
        let source = ImageMetadata {
            format: ImageFormat::WebP,
            has_alpha: true,
            is_lossy: true,
            ..metadata(1, 1)
        };
        let mut args = options(ResizeOperation::Scale, Some(1), Some(1));
        assert_eq!(
            ImageRecipe::resolve(&source, args).unwrap().format(),
            ImageFormat::Png
        );
        args.format = OutputFormat::Jpeg;
        assert!(ImageRecipe::resolve(&source, args).is_err());
        args.background = Some(Background {
            red: 255,
            green: 255,
            blue: 255,
        });
        assert_eq!(
            ImageRecipe::resolve(&source, args).unwrap().format(),
            ImageFormat::Jpeg
        );
    }

    /// An unstated output format follows the source: a lossy photograph stays lossy, and an image
    /// that is neither lossy nor transparent stays a PNG. Resolving the wrong way re-encodes a
    /// photograph as a far larger PNG, and does so without any error.
    #[test]
    fn auto_encoding_follows_the_source() {
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

    fn vector_options(
        operation: ResizeOperation,
        width: Option<u32>,
        height: Option<u32>,
    ) -> ResizeOptions {
        ResizeOptions {
            format: OutputFormat::Png,
            ..options(operation, width, height)
        }
    }

    #[test]
    fn vector_operations_resolve_display_geometry() {
        let source = (300.0, 380.0);
        let render = SvgRender::new(7);
        for (operation, width, height, expected) in GEOMETRY_CASES {
            let recipe =
                ImageRecipe::resolve_svg(source, vector_options(operation, width, height), render)
                    .unwrap();
            assert_eq!((recipe.width(), recipe.height()), expected);
            assert!(recipe.vector_geometry().is_some());
            assert_eq!(recipe.svg_render(), Some(render));
        }
        let fill = ImageRecipe::resolve_svg(
            source,
            vector_options(ResizeOperation::Fill, Some(150), Some(150)),
            render,
        )
        .unwrap();
        let (x, y, width, height) = fill.vector_geometry().unwrap().crop();
        assert_eq!((x, y), (0.0, 40.0));
        assert!((width - 300.0).abs() < 1e-9 && (height - 300.0).abs() < 1e-9);
    }

    #[test]
    fn vector_rejects_auto_format() {
        let mut request = vector_options(ResizeOperation::Scale, Some(10), Some(10));
        request.format = OutputFormat::Auto;
        let refused =
            ImageRecipe::resolve_svg((10.0, 10.0), request, SvgRender::new(1)).unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("cannot choose a format for an SVG"),
            "{refused}"
        );
    }

    /// A recipe serializes the geometry it resizes by — a raster's crop or a vector's own — and
    /// reads back as the recipe it was.
    #[test]
    fn recipes_serialize_their_own_geometry() {
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
                ImageRecipe::resolve_svg(
                    (10.0, 10.0),
                    vector_options(ResizeOperation::Scale, Some(5), Some(5)),
                    SvgRender::new(9),
                )
                .unwrap(),
                &["vector", "svg"][..],
                &["crop"][..],
            ),
        ] {
            let value = serde_json::to_value(&recipe).unwrap();
            for field in present {
                assert!(value.get(field).is_some(), "{field}: {value}");
            }
            for field in absent {
                assert!(value.get(field).is_none(), "{field}: {value}");
            }
            assert_eq!(
                serde_json::from_value::<ImageRecipe>(value).unwrap(),
                recipe
            );
        }
    }

    #[test]
    fn vector_validation_matches_source() {
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
