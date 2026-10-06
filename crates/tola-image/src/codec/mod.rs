//! Source bytes in, one sRGB RGBA8 plane out; resampled pixels in, encoded bytes out.

use std::borrow::Cow;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use image::codecs::bmp::BmpDecoder;
use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::Orientation;
use image::{ColorType, ImageDecoder, Limits};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

use super::cancellation::{Cancellation, ensure_active_if_present};
use super::recipe::PixelRecipe;
use super::resample::{ResampledPixels, resample};
use super::{ImageFormat, ImageMetadata, ImageRecipe, check_dimensions, pixel_bytes};

/// Report what one image's bytes say about themselves, without decoding its pixels.
pub fn inspect(bytes: &[u8]) -> Result<ImageMetadata> {
    if looks_like_xml(bytes) {
        return inspect_svg(bytes);
    }
    Source::metadata(bytes)
}

/// Cancellation takes precedence even when an indivisible codec call also reports an error.
/// Codec errors can wrap I/O errors; callers always receive the original typed `ImageCancelled`.
pub fn render(
    bytes: &[u8],
    recipe: &ImageRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Arc<[u8]>> {
    cancellation.ensure_active()?;
    let result = DecodedSource::open(bytes, cancellation)?.render(recipe, cancellation);
    cancellation.ensure_active()?;
    result
}

/// One decoded image, shared by every recipe resampled from it.
///
/// Color conversion is shared; each recipe applies display orientation to the crop it selects.
pub struct DecodedSource {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    orientation: Orientation,
    metadata: ImageMetadata,
}

impl DecodedSource {
    /// Decode an image into the sRGB RGBA8 pixels every recipe starts from.
    pub fn open(bytes: &[u8], cancellation: &dyn Cancellation) -> Result<Self> {
        cancellation.ensure_active()?;
        let result = Self::open_active(bytes, cancellation);
        cancellation.ensure_active()?;
        result
    }

    fn open_active(bytes: &[u8], cancellation: &dyn Cancellation) -> Result<Self> {
        ensure!(
            !looks_like_xml(bytes),
            "`resize-image` cannot resize an SVG or XML file; use a PNG, JPEG, WebP, GIF, or BMP image"
        );
        let Source {
            mut decoder,
            width,
            height,
            color,
            orientation,
            metadata,
            container,
        } = Source::open(bytes, cancellation)?;
        ensure!(
            !container.is_animated,
            "an animated image cannot be resized without dropping frames; use a still image"
        );
        // Only the declaration the container's own precedence selected is read: an ICC profile a
        // higher declaration supersedes is neither converted nor refused.
        let transform = match container.color {
            SampleColor::Srgb => None,
            SampleColor::Gamma(gamma) => Some(ColorTransform::Gamma(gamma)),
            SampleColor::Unsupported(reason) => bail!("{reason}"),
            SampleColor::StoredIcc(profile) => Some(ColorTransform::Icc(profile)),
            SampleColor::DecoderIcc => Some(ColorTransform::Icc(Cow::Owned(
                decoder
                    .icc_profile()?
                    .context("the embedded color profile is damaged; convert the image to sRGB")?,
            ))),
        };
        // Owned planes share the decoder's budget; backend scratch limits remain best-effort.
        let mut limits = Limits::default();
        limits
            .reserve_buffer(width, height, color)
            .context("the image is too large to decode; use a smaller image")?;
        if color != ColorType::Rgba8 || transform.is_some() {
            limits
                .clone()
                .reserve_buffer(width, height, ColorType::Rgba8)
                .context("the image is too large to decode; use a smaller image")?;
        }
        cancellation.ensure_active()?;
        let raw = decoder.decode(color, width, height, limits)?;
        // The plane every later row walk is cut from must hold exactly the geometry it claims:
        // a short or padded decode would otherwise leave rows paired with the wrong samples.
        ensure!(
            raw.len() == pixel_bytes(width, height, usize::from(color.bytes_per_pixel()))?,
            "the image is damaged or incomplete; re-export it"
        );
        cancellation.ensure_active()?;
        let rgba = normalize_color(raw, color, (width, height), transform, cancellation)?;
        cancellation.ensure_active()?;
        Ok(Self {
            rgba,
            width,
            height,
            orientation,
            metadata,
        })
    }

    /// The decoded image's display dimensions, format, and channels.
    pub fn metadata(&self) -> &ImageMetadata {
        &self.metadata
    }

    /// Resample the pixels one recipe's output geometry asks for.
    ///
    /// Two recipes that share their pixels resample once: encode the result for each of them with
    /// [`ResampledPixels::encode`].
    pub fn pixels(
        &self,
        pixels: PixelRecipe,
        cancellation: &dyn Cancellation,
    ) -> Result<ResampledPixels> {
        cancellation.ensure_active()?;
        pixels.validate(&self.metadata)?;
        let result = resample(
            &self.rgba,
            self.width,
            self.height,
            self.orientation,
            !self.metadata.has_alpha,
            pixels,
            cancellation,
        );
        cancellation.ensure_active()?;
        result
    }

    /// Resample and encode one recipe.
    pub fn render(
        &self,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
    ) -> Result<Arc<[u8]>> {
        cancellation.ensure_active()?;
        recipe.validate(&self.metadata)?;
        let result = self
            .pixels(recipe.pixels(), cancellation)?
            .encode(recipe, cancellation);
        cancellation.ensure_active()?;
        result
    }
}

enum Decoder<'a> {
    Jpeg(Box<zune_jpeg::JpegDecoder<ZCursor<&'a [u8]>>>),
    Raster(Box<dyn ImageDecoder + 'a>),
}

impl Decoder<'_> {
    /// The ICC profile the container's own records carry for the samples this decoder reads.
    ///
    /// Only a raster container declares [`SampleColor::DecoderIcc`]: a JPEG's profile is read
    /// from the header parse that chose its decode layout and arrives as stored bytes.
    fn icc_profile(&mut self) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Raster(decoder) => decoder
                .icc_profile()
                .context("the embedded color profile is damaged; convert the image to sRGB"),
            Self::Jpeg(_) => Ok(None),
        }
    }

    fn decode(
        mut self,
        color: ColorType,
        width: u32,
        height: u32,
        limits: Limits,
    ) -> Result<Vec<u8>> {
        if let Self::Raster(decoder) = &mut self {
            decoder
                .set_limits(limits)
                .context("the image is too large to decode; use a smaller image")?;
        }
        let length = pixel_bytes(width, height, usize::from(color.bytes_per_pixel()))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .context("the image is too large to hold in memory; use a smaller image")?;
        bytes.resize(length, 0);
        match self {
            Self::Jpeg(mut decoder) => decoder
                .decode_into(&mut bytes)
                .context("the JPEG image is damaged or incomplete; re-export it")?,
            Self::Raster(decoder) => decoder
                .read_image(&mut bytes)
                .context("the image is damaged or incomplete; re-export it")?,
        }
        Ok(bytes)
    }
}

struct Source<'a> {
    decoder: Decoder<'a>,
    width: u32,
    height: u32,
    color: ColorType,
    orientation: Orientation,
    metadata: ImageMetadata,
    container: ContainerMetadata<'a>,
}

#[derive(Default)]
struct ContainerMetadata<'a> {
    is_animated: bool,
    is_lossy: bool,
    has_alpha: Option<bool>,
    exif: Option<&'a [u8]>,
    color: SampleColor<'a>,
}

/// The color the container's own records state for its samples.
///
/// This is where a read that only reports metadata stops. A decode reads the ICC profile the
/// variant names, so a profile a higher declaration supersedes never reaches it.
#[derive(Default)]
enum SampleColor<'a> {
    /// The samples already carry sRGB values.
    #[default]
    Srgb,
    /// The ICC profile the container's own records carry governs the samples.
    StoredIcc(Cow<'a, [u8]>),
    /// The ICC profile the pixel decoder unwraps from the container governs the samples.
    DecoderIcc,
    /// The container's gamma exponent governs the samples.
    Gamma(f32),
    /// The container states color Tola cannot resize.
    Unsupported(&'static str),
}

impl<'a> Source<'a> {
    /// Report what one image's bytes say about themselves, without decoding its pixels.
    ///
    /// A metadata read is a bounded header pass: it takes no cancellation token, and the color
    /// refusals a decode runs into on its way to pixels never reach it.
    fn metadata(bytes: &'a [u8]) -> Result<ImageMetadata> {
        let format = image_format(bytes)?;
        if format == ImageFormat::Jpeg {
            let (_, header) = Self::jpeg_header(bytes)?;
            let (display_width, display_height) =
                display_dimensions(header.width, header.height, header.orientation);
            return Ok(ImageMetadata {
                width: display_width,
                height: display_height,
                format: ImageFormat::Jpeg,
                has_alpha: false,
                is_lossy: true,
            });
        }
        Ok(Self::raster(bytes, format, None)?.metadata)
    }

    fn open(bytes: &'a [u8], cancellation: &dyn Cancellation) -> Result<Self> {
        cancellation.ensure_active()?;
        let format = image_format(bytes)?;
        if format == ImageFormat::Jpeg {
            return Self::jpeg(bytes, cancellation);
        }
        Self::raster(bytes, format, Some(cancellation))
    }

    /// Build one raster container's decoder from the scan of its own header records.
    ///
    /// Every raster format scans and constructs its decoder here; only the cancellation token a
    /// metadata read does not carry differs between the two entries.
    fn raster(
        bytes: &'a [u8],
        format: ImageFormat,
        cancellation: Option<&dyn Cancellation>,
    ) -> Result<Self> {
        ensure_active_if_present(cancellation)?;
        let container = match format {
            ImageFormat::Png => scan_png(bytes, cancellation)?,
            ImageFormat::WebP => scan_webp(bytes, cancellation)?,
            ImageFormat::Gif => scan_gif(bytes, cancellation)?,
            ImageFormat::Bmp => scan_bmp(bytes)?,
            ImageFormat::Jpeg | ImageFormat::Svg => bail!(
                "this image format is not supported; save the image as PNG, JPEG, WebP, GIF, or BMP"
            ),
        };
        ensure_active_if_present(cancellation)?;
        let mut decoder: Box<dyn ImageDecoder + 'a> = match format {
            ImageFormat::Png => Box::new(
                PngDecoder::with_limits(Cursor::new(bytes), Limits::default())
                    .context("the PNG image is damaged or incomplete; re-export it")?,
            ),
            ImageFormat::WebP => Box::new(
                WebPDecoder::new(Cursor::new(bytes))
                    .context("the WebP image is damaged or incomplete; re-export it")?,
            ),
            ImageFormat::Gif => Box::new(
                GifDecoder::new(Cursor::new(bytes))
                    .context("the GIF image is damaged or incomplete; re-export it")?,
            ),
            ImageFormat::Bmp => Box::new(
                BmpDecoder::new(Cursor::new(bytes))
                    .context("the BMP image is damaged or incomplete; re-export it")?,
            ),
            ImageFormat::Jpeg | ImageFormat::Svg => bail!(
                "this image format is not supported; save the image as PNG, JPEG, WebP, GIF, or BMP"
            ),
        };
        decoder
            .set_limits(Limits::default())
            .context("the image is too large to decode; use a smaller image")?;
        let (width, height) = decoder.dimensions();
        check_dimensions(width, height)?;
        let color = decoder.color_type();
        let orientation = container
            .exif
            .map(exif_orientation)
            .unwrap_or(Orientation::NoTransforms);
        let (display_width, display_height) = display_dimensions(width, height, orientation);
        Ok(Self {
            decoder: Decoder::Raster(decoder),
            width,
            height,
            color,
            orientation,
            metadata: ImageMetadata {
                width: display_width,
                height: display_height,
                format,
                has_alpha: container.has_alpha.unwrap_or_else(|| color.has_alpha()),
                is_lossy: container.is_lossy,
            },
            container,
        })
    }

    fn jpeg(bytes: &'a [u8], cancellation: &dyn Cancellation) -> Result<Self> {
        let (parsed, header) = Self::jpeg_header(bytes)?;
        // The byte walk only reports a profile its own marker chain shows it; the profile zune
        // assembles from the same headers owns the layout and the conversion, so a walk that
        // mis-parses the chain may refuse a file but never drop color management.
        let declared = jpeg_has_icc(bytes, cancellation)?;
        let profile = parsed.icc_profile();
        ensure!(
            !declared || profile.is_some(),
            "the embedded color profile is damaged; convert the image to sRGB"
        );
        let icc = profile.is_some();
        let container = ContainerMetadata {
            is_lossy: true,
            color: match profile {
                Some(profile) => SampleColor::StoredIcc(Cow::Owned(profile)),
                None => SampleColor::Srgb,
            },
            ..ContainerMetadata::default()
        };
        cancellation.ensure_active()?;
        // A profile is converted from the source layout. Without one the decoder can add opaque
        // alpha while converting YCbCr or luma, but zune-jpeg has no RGB-to-RGBA conversion.
        let (color, output_space) =
            if !icc && matches!(header.input_space, ColorSpace::YCbCr | ColorSpace::Luma) {
                (ColorType::Rgba8, ColorSpace::RGBA)
            } else if header.color == ColorType::L8 {
                (header.color, ColorSpace::Luma)
            } else {
                (header.color, ColorSpace::RGB)
            };
        // zune-jpeg picks its color conversion while it parses headers, so the output colorspace
        // belongs to the decoder that decodes and must be set before it sees them; the parse above
        // only reports the source's own layout.
        let decoder = zune_jpeg::JpegDecoder::new_with_options(
            ZCursor::new(bytes),
            jpeg_options().jpeg_set_out_colorspace(output_space),
        );
        let (display_width, display_height) =
            display_dimensions(header.width, header.height, header.orientation);
        Ok(Self {
            decoder: Decoder::Jpeg(Box::new(decoder)),
            width: header.width,
            height: header.height,
            color,
            orientation: header.orientation,
            metadata: ImageMetadata {
                width: display_width,
                height: display_height,
                format: ImageFormat::Jpeg,
                has_alpha: false,
                is_lossy: true,
            },
            container,
        })
    }

    /// Parse one JPEG's headers, sharing every interpretation a metadata read and a decode use.
    ///
    /// The decoder returned holds those headers; a decode reads the ICC profile they assembled
    /// from it, while a metadata read takes only the facts they state.
    fn jpeg_header(
        bytes: &'a [u8],
    ) -> Result<(zune_jpeg::JpegDecoder<ZCursor<&'a [u8]>>, JpegHeader)> {
        // The image crate converts CMYK/YCCK silently and hides the original model. Use its
        // maintained JPEG backend directly so unsupported models cannot lose their color meaning.
        let mut decoder =
            zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), jpeg_options());
        decoder
            .decode_headers()
            .context("the JPEG image is damaged or incomplete; re-export it")?;
        let (width, height) = decoder
            .dimensions()
            .context("the JPEG has no usable dimensions; re-export it")?;
        let (width, height) = (
            u32::try_from(width)
                .context("the JPEG dimensions are too large to resize; use a smaller image")?,
            u32::try_from(height)
                .context("the JPEG dimensions are too large to resize; use a smaller image")?,
        );
        check_dimensions(width, height)?;
        let input_space = decoder
            .input_colorspace()
            .context("the JPEG has no usable color information; re-export it as RGB")?;
        let color = match input_space {
            ColorSpace::Luma => ColorType::L8,
            ColorSpace::RGB | ColorSpace::YCbCr => ColorType::Rgb8,
            _ => bail!("CMYK and YCCK JPEG images are not supported; re-export the image as RGB"),
        };
        let orientation = decoder
            .exif()
            .map(|bytes| exif_orientation(bytes))
            .unwrap_or(Orientation::NoTransforms);
        Ok((
            decoder,
            JpegHeader {
                width,
                height,
                color,
                input_space,
                orientation,
            },
        ))
    }
}

/// What one JPEG's parsed headers state about the image: its sample layout and pixel geometry,
/// and the orientation the display dimensions follow from.
struct JpegHeader {
    width: u32,
    height: u32,
    color: ColorType,
    input_space: ColorSpace,
    orientation: Orientation,
}

/// The zune-jpeg options every read of one source uses. Tola applies its own dimension limits, so
/// the parse disables the backend's.
fn jpeg_options() -> DecoderOptions {
    DecoderOptions::default()
        .set_use_unsafe(false)
        .set_strict_mode(true)
        .set_max_width(usize::MAX)
        .set_max_height(usize::MAX)
}

/// The format one image's bytes declare, refused when Tola cannot read it.
fn image_format(bytes: &[u8]) -> Result<ImageFormat> {
    match image::guess_format(bytes)
        .context("the file is not an image Tola can read; use PNG, JPEG, WebP, GIF, BMP, or SVG")?
    {
        image::ImageFormat::Jpeg => Ok(ImageFormat::Jpeg),
        image::ImageFormat::Png => Ok(ImageFormat::Png),
        image::ImageFormat::WebP => Ok(ImageFormat::WebP),
        image::ImageFormat::Gif => Ok(ImageFormat::Gif),
        image::ImageFormat::Bmp => Ok(ImageFormat::Bmp),
        _ => bail!(
            "this image format is not supported; save the image as PNG, JPEG, WebP, GIF, or BMP"
        ),
    }
}

fn exif_orientation(bytes: &[u8]) -> Orientation {
    let tiff = bytes.strip_prefix(b"Exif\0\0").unwrap_or(bytes);
    Orientation::from_exif_chunk(tiff).unwrap_or(Orientation::NoTransforms)
}

fn display_dimensions(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (height, width),
        _ => (width, height),
    }
}
use self::color::{ColorTransform, normalize_color};
use self::container::{
    inspect_svg, jpeg_has_icc, looks_like_xml, scan_bmp, scan_gif, scan_png, scan_webp,
};

mod color;
mod container;
mod encode;

/// Encoders and sample images the codec module's tests share.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::ImageRecipe;
    use crate::{OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions};
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    /// A cancellation request that never fires, so a test exercises the byte pipeline alone.
    pub(super) fn active() -> bool {
        false
    }

    pub(super) fn png(
        pixels: &[u8],
        width: u32,
        height: u32,
        color: ExtendedColorType,
        icc: Option<Vec<u8>>,
        exif: Option<Vec<u8>>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = PngEncoder::new(&mut bytes);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc).unwrap();
        }
        if let Some(exif) = exif {
            encoder.set_exif_metadata(exif).unwrap();
        }
        encoder.write_image(pixels, width, height, color).unwrap();
        bytes
    }

    /// An sRGB ICC profile whose transfer curves are linear, so a grey converts to near 188
    /// rather than staying on 128.
    pub(super) fn linear_profile() -> Vec<u8> {
        let mut profile = moxcms::ColorProfile::new_srgb();
        // Without this the encoded profile still describes sRGB, and the curves below do not
        // govern the transform.
        profile.cicp = None;
        let curve = moxcms::curve_from_gamma(1.0);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
        profile.encode().unwrap()
    }

    /// One flat grey 3x3 JPEG carrying `linear_profile`, so a converted pixel lands near 188 and
    /// a profile the decode drops leaves it on 128.
    pub(super) fn linear_icc_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 100);
        encoder.set_icc_profile(linear_profile()).unwrap();
        encoder
            .write_image(&[128; 3 * 3 * 3], 3, 3, ExtendedColorType::Rgb8)
            .unwrap();
        jpeg
    }

    pub(super) fn recipe(
        bytes: &[u8],
        width: u32,
        height: u32,
        format: OutputFormat,
        quality: Option<u8>,
    ) -> ImageRecipe {
        ImageRecipe::resolve(
            &inspect(bytes).unwrap(),
            ResizeOptions {
                width: Some(width),
                height: Some(height),
                operation: ResizeOperation::Scale,
                format,
                quality,
                filter: ResizeFilter::Triangle,
                background: None,
            },
        )
        .unwrap()
    }

    /// Assemble one PNG chunk: length, type, data, and the CRC the format carries.
    pub(super) fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(kind);
        hasher.update(data);
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&hasher.finalize().to_be_bytes());
        chunk
    }

    /// Insert a chunk directly after the image header, where the format still lets a reader
    /// honour it.
    pub(super) fn insert_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        let header_bytes =
            u32::from_be_bytes(png[8..12].try_into().expect("a chunk length")) as usize;
        let at = 8 + 12 + header_bytes;
        png.splice(at..at, png_chunk(kind, data));
    }

    pub(super) fn orientation_exif(orientation: u8) -> Vec<u8> {
        vec![
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0,
            0x12,
            1,
            3,
            0,
            1,
            0,
            0,
            0,
            orientation,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]
    }

    pub(super) fn rgba(bytes: &[u8]) -> Vec<u8> {
        image::load_from_memory(bytes)
            .unwrap()
            .into_rgba8()
            .into_raw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputFormat;
    use crate::cancellation::ImageCancelled;
    use crate::codec::test_support::*;
    use crate::codec::{inspect, render};
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::jpeg::JpegEncoder;

    #[test]
    fn exif_orientation_is_applied_once() {
        let pixels: Vec<u8> = (1..=6).flat_map(|value| [value, 0, 0, 255]).collect();
        for (orientation, expected) in [
            (1, [1, 2, 3, 4, 5, 6]),
            (2, [3, 2, 1, 6, 5, 4]),
            (3, [6, 5, 4, 3, 2, 1]),
            (4, [4, 5, 6, 1, 2, 3]),
            (5, [1, 4, 2, 5, 3, 6]),
            (6, [4, 1, 5, 2, 6, 3]),
            (7, [6, 3, 5, 2, 4, 1]),
            (8, [3, 6, 2, 5, 1, 4]),
        ] {
            let source = png(
                &pixels,
                3,
                2,
                ExtendedColorType::Rgba8,
                None,
                Some(orientation_exif(orientation)),
            );
            let metadata = inspect(&source).unwrap();
            let dimensions = if orientation >= 5 { (2, 3) } else { (3, 2) };
            assert_eq!((metadata.width, metadata.height), dimensions);
            let recipe = recipe(&source, dimensions.0, dimensions.1, OutputFormat::Png, None);
            let result = render(&source, &recipe, &active).unwrap();
            let displayed: Vec<_> = rgba(&result)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[0])
                .collect();
            assert_eq!(displayed, expected);
            let mut decoder = PngDecoder::new(Cursor::new(&result[..])).unwrap();
            assert_eq!(decoder.orientation().unwrap(), Orientation::NoTransforms);
            assert!(decoder.exif_metadata().unwrap().is_none());
        }
    }

    #[test]
    fn jpeg_dimensions_account_for_orientation() {
        let mut source = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut source, 90);
        encoder.set_exif_metadata(orientation_exif(6)).unwrap();
        encoder
            .write_image(&[120; 3 * 2 * 3], 3, 2, ExtendedColorType::Rgb8)
            .unwrap();
        assert_eq!(
            (
                inspect(&source).unwrap().width,
                inspect(&source).unwrap().height
            ),
            (2, 3)
        );
        let recipe = recipe(&source, 2, 3, OutputFormat::Jpeg, Some(90));
        let encoded = render(&source, &recipe, &active).unwrap();
        let metadata = inspect(&encoded).unwrap();
        assert_eq!(
            (metadata.width, metadata.height, metadata.format),
            (2, 3, ImageFormat::Jpeg)
        );
        let mut decoded = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&encoded[..])).unwrap();
        assert!(decoded.exif_metadata().unwrap().is_none());
    }

    /// The byte range of the APP2 segment carrying `linear_icc_jpeg`'s profile, which fits one
    /// segment.
    fn icc_app2_span(jpeg: &[u8]) -> std::ops::Range<usize> {
        let mut signatures = jpeg
            .windows(12)
            .enumerate()
            .filter(|(_, window)| *window == b"ICC_PROFILE\0")
            .map(|(at, _)| at - 4);
        let at = signatures
            .next()
            .expect("the encoded profile's APP2 segment");
        assert!(
            signatures.next().is_none(),
            "the profile fits one APP2 segment"
        );
        let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        at..at + 2 + length
    }

    /// The same JPEG with `FF 00` stuffing inserted before its APP2 segment, and a COM segment as
    /// long as the bytes the byte walk then reads as that segment's length.
    fn jpeg_stuffed_before_icc(jpeg: &[u8]) -> Vec<u8> {
        let span = icc_app2_span(jpeg);
        // `FF 00` before the segment makes the walk read the segment's own `FF E2` marker as a
        // 65506-byte length, so the stuffing only stays inside the file while the COM segment
        // ends that mis-read exactly on the marker that follows the profile.
        let footer = 65_506 - span.len();
        let mut stuffed = Vec::with_capacity(jpeg.len() + footer + 2);
        stuffed.extend_from_slice(&jpeg[..span.start]);
        stuffed.extend_from_slice(&[0xff, 0x00]);
        stuffed.extend_from_slice(&jpeg[span.clone()]);
        stuffed.extend_from_slice(&[0xff, 0xfe]);
        stuffed.extend_from_slice(&(footer as u16 - 2).to_be_bytes());
        stuffed.resize(stuffed.len() + footer - 4, 0);
        stuffed.extend_from_slice(&jpeg[span.end..]);
        stuffed
    }

    /// The same JPEG with `FF 00` stuffing inserted right after its APP2 segment, where the byte
    /// walk reads the next marker as the segment's length.
    fn jpeg_stuffed_after_icc(jpeg: &[u8]) -> Vec<u8> {
        let span = icc_app2_span(jpeg);
        let mut stuffed = Vec::with_capacity(jpeg.len() + 2);
        stuffed.extend_from_slice(&jpeg[..span.end]);
        stuffed.extend_from_slice(&[0xff, 0x00]);
        stuffed.extend_from_slice(&jpeg[span.end..]);
        stuffed
    }

    /// A profile the byte walk mis-parses is still the profile zune reads from the same headers:
    /// the stuffing changes no pixel.
    #[test]
    fn jpeg_padding_keeps_the_embedded_profile() {
        let clean = linear_icc_jpeg();
        let stuffed = jpeg_stuffed_before_icc(&clean);
        let clean_pixels = rgba(
            &render(
                &clean,
                &recipe(&clean, 3, 3, OutputFormat::Png, None),
                &active,
            )
            .unwrap(),
        );
        assert!(
            (180..=195).contains(&clean_pixels[0]),
            "the embedded profile is applied: {clean_pixels:?}"
        );
        let stuffed_pixels = rgba(
            &render(
                &stuffed,
                &recipe(&stuffed, 3, 3, OutputFormat::Png, None),
                &active,
            )
            .unwrap(),
        );
        assert_eq!(stuffed_pixels, clean_pixels);
    }

    /// Stuffing the header parse tolerates is still damage to the byte walk: the metadata read
    /// succeeds where resizing the same bytes refuses.
    #[test]
    fn jpeg_padding_still_reports_metadata() {
        let clean = linear_icc_jpeg();
        let stuffed = jpeg_stuffed_after_icc(&clean);
        let metadata = inspect(&stuffed).unwrap();
        assert_eq!((metadata.width, metadata.height), (3, 3));
        let refusal = render(
            &stuffed,
            &recipe(&stuffed, 3, 3, OutputFormat::Png, None),
            &active,
        )
        .unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "the JPEG image is damaged or incomplete; re-export it"
        );
    }

    #[test]
    fn rgb_jpeg_retains_its_channels() {
        // Pillow's keep_rgb stores RGB component IDs rather than YCbCr samples.
        let source = &[
            0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x00, 0x64, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x01, 0x01, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x01, 0x00, 0x01, 0x03, 0x52,
            0x11, 0x00, 0x47, 0x11, 0x00, 0x42, 0x11, 0x00, 0xff, 0xc4, 0x00, 0x15, 0x00, 0x01,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x0a, 0x00, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xda, 0x00,
            0x0c, 0x03, 0x52, 0x00, 0x47, 0x00, 0x42, 0x00, 0x00, 0x3f, 0x00, 0x1f, 0xe8, 0xc0,
            0x1f, 0xff, 0xd9,
        ];
        let png = render(
            source,
            &recipe(source, 1, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        assert_eq!(rgba(&png), [32, 128, 224, 255]);
    }

    #[test]
    fn premultiplied_filtering_avoids_bleed() {
        let source = png(
            &[255, 0, 0, 255, 0, 255, 0, 0],
            2,
            1,
            ExtendedColorType::Rgba8,
            None,
            None,
        );
        let recipe = recipe(&source, 1, 1, OutputFormat::Png, None);
        let output = render(&source, &recipe, &active).unwrap();
        assert_eq!(rgba(&output), [255, 0, 0, 128]);
    }

    #[test]
    fn malformed_images_are_rejected() {
        for bytes in [
            b"".as_slice(),
            b"not an image",
            b"\xff\xd8\xff",
            b"GIF89a",
            b"\x89PNG\r\n\x1a\n",
            b"RIFF\xff\xff\xff\xffWEBP",
        ] {
            assert!(inspect(bytes).is_err());
        }
        let source = png(&[255, 0, 0], 1, 1, ExtendedColorType::Rgb8, None, None);
        let recipe = recipe(&source, 1, 1, OutputFormat::Png, None);
        assert!(render(&source[..source.len() - 5], &recipe, &active).is_err());
    }

    #[test]
    fn exif_after_pixel_data_is_applied() {
        let mut source = png(
            &[1, 0, 0, 2, 0, 0],
            2,
            1,
            ExtendedColorType::Rgb8,
            None,
            None,
        );
        let exif = orientation_exif(6);
        let before_iend = source.len() - 12;
        source.splice(before_iend..before_iend, png_chunk(b"eXIf", &exif));
        let metadata = inspect(&source).unwrap();
        assert_eq!((metadata.width, metadata.height), (1, 2));
        let recipe = recipe(&source, 1, 2, OutputFormat::Png, None);
        let output = render(&source, &recipe, &active).unwrap();
        assert_eq!(rgba(&output), [1, 0, 0, 255, 2, 0, 0, 255]);
        source[before_iend + 8 + 18] = 8;
        assert!(inspect(&source).is_err());
        assert!(render(&source, &recipe, &active).is_err());
    }

    #[test]
    fn cancellation_precedes_decode_errors() {
        let source = png(&[10, 20, 30], 1, 1, ExtendedColorType::Rgb8, None, None);
        let recipe = recipe(&source, 1, 1, OutputFormat::Png, None);
        let cancellation = || true;
        assert!(
            render(b"broken source", &recipe, &cancellation)
                .unwrap_err()
                .is::<ImageCancelled>()
        );
        assert!(
            render(&source, &recipe, &cancellation)
                .unwrap_err()
                .is::<ImageCancelled>()
        );
    }
}
