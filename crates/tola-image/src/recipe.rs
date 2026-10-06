use anyhow::{Result, ensure};
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
    /// Exactly the requested size, cropped around the source's center, or enlarged when the
    /// source is smaller than the request.
    Fill,
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

    fn centered(metadata: &ImageMetadata, width: u32, height: u32) -> Self {
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
        Self {
            x: (metadata.width - crop_width) / 2,
            y: (metadata.height - crop_height) / 2,
            width: crop_width,
            height: crop_height,
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
    pub(super) crop: Crop,
    pub(super) filter: ResizeFilter,
    pub(super) background: Option<Background>,
}

impl PixelRecipe {
    /// The width these pixels are resampled to.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// The height these pixels are resampled to.
    pub const fn height(self) -> u32 {
        self.height
    }

    fn normalize_filter(&mut self) {
        // Copying an unchanged crop bypasses convolution, regardless of the requested filter.
        if (self.width, self.height) == (self.crop.width, self.crop.height) {
            self.filter = ResizeFilter::default();
        }
    }

    fn validate_structure(self) -> Result<()> {
        check_dimensions(self.width, self.height)?;
        check_dimensions(self.crop.width, self.crop.height)?;
        ensure!(
            self.crop.x.checked_add(self.crop.width).is_some()
                && self.crop.y.checked_add(self.crop.height).is_some(),
            "image crop coordinates overflow"
        );
        Ok(())
    }

    /// Whether these pixels can be resampled from an image with this metadata.
    pub fn validate(self, metadata: &ImageMetadata) -> Result<()> {
        self.validate_structure()?;
        check_dimensions(metadata.width, metadata.height)?;
        ensure!(
            metadata.format != ImageFormat::Svg,
            "`resize-image` cannot resize an SVG; use a PNG, JPEG, WebP, GIF, or BMP image"
        );
        ensure!(
            self.crop.x + self.crop.width <= metadata.width
                && self.crop.y + self.crop.height <= metadata.height,
            "the resize crop is outside the source image"
        );
        ensure!(
            self.crop == Crop::full(metadata)
                || self.crop == Crop::centered(metadata, self.width, self.height),
            "image crop must be the complete source or its canonical centered fill crop"
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
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeFields {
    width: u32,
    height: u32,
    crop: Crop,
    filter: ResizeFilter,
    background: Option<Background>,
    encoding: Encoding,
}

impl From<ImageRecipe> for RecipeFields {
    fn from(recipe: ImageRecipe) -> Self {
        Self {
            width: recipe.pixels.width,
            height: recipe.pixels.height,
            crop: recipe.pixels.crop,
            filter: recipe.pixels.filter,
            background: recipe.pixels.background,
            encoding: recipe.encoding,
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
                filter: fields.filter,
                background: fields.background,
            },
            encoding: fields.encoding,
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
        ensure!(
            metadata.format != ImageFormat::Svg,
            "`resize-image` cannot resize an SVG; use a PNG, JPEG, WebP, GIF, or BMP image"
        );
        for dimension in [options.width, options.height].into_iter().flatten() {
            ensure!(
                dimension > 0,
                "`width` and `height` must be positive pixel counts"
            );
        }
        let required_width = || {
            options.width.ok_or_else(|| {
                anyhow::anyhow!("`resize-image` needs `width` for this `op`; pass `width`")
            })
        };
        let required_height = || {
            options.height.ok_or_else(|| {
                anyhow::anyhow!("`resize-image` needs `height` for this `op`; pass `height`")
            })
        };
        let mut crop = Crop::full(metadata);
        let (width, height) = match options.operation {
            ResizeOperation::Scale => (required_width()?, required_height()?),
            ResizeOperation::FitWidth => {
                let width = required_width()?;
                (
                    width,
                    scaled_dimension(metadata.height, width, metadata.width)?,
                )
            }
            ResizeOperation::FitHeight => {
                let height = required_height()?;
                (
                    scaled_dimension(metadata.width, height, metadata.height)?,
                    height,
                )
            }
            ResizeOperation::Fit => {
                let width = required_width()?;
                let height = required_height()?;
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
                let width = required_width()?;
                let height = required_height()?;
                crop = Crop::centered(metadata, width, height);
                (width, height)
            }
        };
        let format = match options.format {
            OutputFormat::Auto if metadata.is_lossy && !metadata.has_alpha => OutputFormat::Jpeg,
            OutputFormat::Auto => OutputFormat::Png,
            format => format,
        };
        let encoding = match format {
            OutputFormat::Jpeg => Encoding::Jpeg {
                quality: options.quality.unwrap_or(75),
            },
            OutputFormat::Png => Encoding::Png,
            OutputFormat::WebP => Encoding::WebP {
                quality: options.quality,
            },
            OutputFormat::Auto => unreachable!("auto encoding was resolved"),
        };
        let mut recipe = Self {
            pixels: PixelRecipe {
                width,
                height,
                crop,
                filter: options.filter,
                background: options.background.filter(|_| metadata.has_alpha),
            },
            encoding,
        };
        recipe.pixels.normalize_filter();
        recipe.validate(metadata)?;
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

    fn validate_structure(&self) -> Result<()> {
        self.pixels.validate_structure()?;
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

    /// Whether a source with this metadata can be rendered by this recipe.
    pub fn validate(&self, metadata: &ImageMetadata) -> Result<()> {
        self.validate_structure()?;
        self.pixels.validate(metadata)?;
        ensure!(
            self.format() != ImageFormat::Jpeg
                || !metadata.has_alpha
                || self.pixels.background.is_some(),
            "JPEG output cannot keep transparency; set `background`, or use `format: \"png\"` or `format: \"webp\"`"
        );
        Ok(())
    }
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

    fn options(
        operation: ResizeOperation,
        width: Option<u32>,
        height: Option<u32>,
    ) -> ResizeOptions {
        ResizeOptions {
            width,
            height,
            operation,
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
        for (operation, width, height, expected) in [
            (ResizeOperation::Scale, Some(150), Some(150), (150, 150)),
            (ResizeOperation::FitWidth, Some(100), None, (100, 127)),
            (ResizeOperation::FitHeight, None, Some(150), (118, 150)),
            (ResizeOperation::Fit, Some(150), Some(150), (118, 150)),
            (ResizeOperation::Fill, Some(150), Some(150), (150, 150)),
        ] {
            let recipe = ImageRecipe::resolve(&source, options(operation, width, height)).unwrap();
            assert_eq!((recipe.width(), recipe.height()), expected);
        }
        let fill = ImageRecipe::resolve(
            &source,
            options(ResizeOperation::Fill, Some(150), Some(150)),
        )
        .unwrap();
        assert_eq!(
            fill.pixels.crop,
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

    #[test]
    fn unchanged_crop_ignores_filter() {
        let source = metadata(9, 6);
        for (operation, width, height) in [
            (ResizeOperation::Fit, 12, 12),
            (ResizeOperation::Fill, 6, 6),
        ] {
            let mut request = options(operation, Some(width), Some(height));
            let recipe = ImageRecipe::resolve(&source, request).unwrap();
            request.filter = ResizeFilter::Triangle;
            assert_eq!(recipe, ImageRecipe::resolve(&source, request).unwrap());
        }
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
    fn crop_stays_centered_inside_the_source() {
        let source = metadata(9, 6);
        let recipe =
            ImageRecipe::resolve(&source, options(ResizeOperation::Fill, Some(4), Some(4)))
                .unwrap();
        assert_eq!(
            recipe.pixels.crop,
            Crop {
                x: 1,
                y: 0,
                width: 6,
                height: 6
            }
        );
        assert!(recipe.validate(&metadata(4, 4)).is_err());
        let mut forged = recipe;
        forged.pixels.crop.x = 0;
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

    /// An SVG is measured but never resampled. Both entry points guard this, because a recipe can
    /// reach `validate` without passing `resolve`.
    #[test]
    fn svg_sources_are_rejected() {
        let svg = ImageMetadata {
            format: ImageFormat::Svg,
            ..metadata(10, 10)
        };
        let request = options(ResizeOperation::Scale, Some(5), Some(5));
        let refused = ImageRecipe::resolve(&svg, request).unwrap_err();
        assert!(
            refused.to_string().contains("cannot resize an SVG"),
            "{refused}"
        );

        let recipe = ImageRecipe::resolve(&metadata(10, 10), request).unwrap();
        let refused = recipe.pixels().validate(&svg).unwrap_err();
        assert!(
            refused.to_string().contains("cannot resize an SVG"),
            "{refused}"
        );
    }
}
