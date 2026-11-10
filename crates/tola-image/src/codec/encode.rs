//! Encoding resampled pixels into the format a recipe asks for.
use std::io::{self, IoSlice, Write};
use std::sync::Arc;

use image::{ExtendedColorType, ImageEncoder};

use crate::cancellation::Cancellation;
use crate::recipe::Encoding;
use crate::resample::ResampledPixels;
use crate::{ImageError, ImageMetadata, ImageRecipe, ImageResult, SizeLimit, zeroed_plane};

/// How a failed WebP encode reads to a site author.
const WEBP_ENCODE_FAILURE: &str = "Tola could not encode this image as WebP; use `format: \"png\"`";

/// How a failed encode reads to a site author.
const ENCODE_FAILURE: &str = "Tola could not encode the resized image; try another `format`";

const PNG_IDAT_BYTES: usize = 64 * 1024;

struct EncodeWriter<'a, W> {
    destination: W,
    cancellation: &'a dyn Cancellation,
}

impl<W: Write> Write for EncodeWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancellation
            .ensure_active()
            .map_err(io::Error::other)?;
        self.destination.write(bytes)
    }

    fn write_vectored(&mut self, chunks: &[IoSlice<'_>]) -> io::Result<usize> {
        self.cancellation
            .ensure_active()
            .map_err(io::Error::other)?;
        self.destination.write_vectored(chunks)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.cancellation
            .ensure_active()
            .map_err(io::Error::other)?;
        self.destination.flush()
    }
}

/// The stream writes its last partial IDAT on drop. Keep a failure there visible to IEND.
struct PngOutput<'a, W> {
    writer: &'a mut W,
    failure: Option<io::ErrorKind>,
}

impl<W: Write> Write for PngOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(kind) = self.failure {
            return Err(kind.into());
        }
        self.writer.write_all(bytes).inspect_err(|error| {
            self.failure = Some(error.kind());
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl ImageRecipe {
    /// Estimated temporary bytes for in-memory encoding, excluding the caller's RGBA plane.
    /// Compressed lengths vary by content. A background makes the output opaque.
    pub fn estimated_encoding_scratch_bytes(
        &self,
        workers: usize,
        may_have_transparency: bool,
    ) -> u64 {
        let pixels = u64::from(self.width()) * u64::from(self.height());
        let transparent = may_have_transparency && self.pixels().background.is_none();
        match self.encoding {
            // The writer and shared output can coexist while the output is published.
            Encoding::Jpeg { .. } => pixels.saturating_mul(2),
            Encoding::Png => {
                // The stream holds three scanlines. Opaque output also converts one RGB row.
                let rows = u64::from(self.width()) * 12;
                let channels = if transparent { 4 } else { 3 };
                pixels
                    .saturating_mul(2 * channels)
                    .saturating_add(rows + PNG_IDAT_BYTES as u64)
            }
            // VP8L also keeps a mutable four-channel transform buffer.
            Encoding::WebP { quality: None } => pixels.saturating_mul(12),
            Encoding::WebP { quality: Some(_) } => tola_webp::estimated_scratch_bytes(
                self.width(),
                self.height(),
                workers,
                transparent,
            ),
        }
    }
}

/// Encode resampled pixels once per recipe that asks for them.
impl ResampledPixels {
    /// Encode these pixels as one recipe asks.
    ///
    /// The recipe must ask for exactly these pixels; a build that resampled once for several
    /// encodings calls this for each of them.
    pub fn encode(
        &self,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
    ) -> ImageResult<Arc<[u8]>> {
        self.encode_bytes(recipe, cancellation, None)
    }

    /// Encode with a maximum worker count, including the calling thread.
    pub fn encode_with_workers(
        &self,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
        workers: usize,
    ) -> ImageResult<Arc<[u8]>> {
        self.encode_bytes(recipe, cancellation, Some(workers))
    }

    fn encode_bytes(
        &self,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
        workers: Option<usize>,
    ) -> ImageResult<Arc<[u8]>> {
        let mut bytes = Vec::new();
        self.write_encoded(&mut bytes, recipe, cancellation, workers)?;
        let encoded = bytes.into();
        cancellation.ensure_active()?;
        Ok(encoded)
    }

    /// Encode to a caller-owned destination without retaining another encoded output here.
    /// Codecs can retain compressed chunks before writing the container.
    pub fn encode_to(
        &self,
        destination: impl Write,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
    ) -> ImageResult<ImageMetadata> {
        self.write_encoded(destination, recipe, cancellation, None)
    }

    fn write_encoded(
        &self,
        destination: impl Write,
        recipe: &ImageRecipe,
        cancellation: &dyn Cancellation,
        workers: Option<usize>,
    ) -> ImageResult<ImageMetadata> {
        cancellation.ensure_active()?;
        if recipe.pixels() != self.pixels() {
            return Err(ImageError::unreadable(
                "the image could not be encoded at this size; try another `width` or `height`",
            ));
        }
        let mut writer = EncodeWriter {
            destination,
            cancellation,
        };
        let result = match recipe.encoding {
            Encoding::Jpeg { quality } if !self.has_transparency() => encode_jpeg(
                &mut writer,
                self.rgba(),
                recipe.width(),
                recipe.height(),
                quality,
            ),
            Encoding::Jpeg { .. } => Err(ImageError::unreadable(ENCODE_FAILURE)),
            Encoding::Png => encode_png(&mut writer, self, cancellation),
            Encoding::WebP { quality } => {
                encode_webp(&mut writer, self, quality, workers, cancellation)
            }
        };
        cancellation.ensure_active()?;
        result?;
        Ok(ImageMetadata {
            width: recipe.width(),
            height: recipe.height(),
            format: recipe.format(),
            has_alpha: self.has_transparency(),
            is_lossy: matches!(
                recipe.encoding,
                Encoding::Jpeg { .. } | Encoding::WebP { quality: Some(_) }
            ),
        })
    }
}

fn encode_webp(
    writer: &mut impl Write,
    pixels: &ResampledPixels,
    quality: Option<u8>,
    workers: Option<usize>,
    cancellation: &dyn Cancellation,
) -> ImageResult<()> {
    let geometry = pixels.pixels();
    let Some(quality) = quality else {
        return image::codecs::webp::WebPEncoder::new_lossless(writer)
            .write_image(
                pixels.rgba(),
                geometry.width(),
                geometry.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|error| match error {
                image::ImageError::IoError(error) => ImageError::codec(ENCODE_FAILURE, error),
                error => ImageError::codec(WEBP_ENCODE_FAILURE, error),
            });
    };

    let mut options = workers.map_or_else(
        || tola_webp::EncodeOptions::new(quality),
        |workers| tola_webp::EncodeOptions::with_workers(quality, workers),
    );
    options.has_transparency = Some(pixels.has_transparency());
    tola_webp::encode_rgba_to(
        pixels.rgba(),
        geometry.width(),
        geometry.height(),
        options,
        &|| cancellation.is_cancelled(),
        writer,
    )
    .map_err(|error| match error {
        tola_webp::WriteError::Encode(tola_webp::Error::Cancelled) => crate::ImageCancelled.into(),
        tola_webp::WriteError::Encode(error) => ImageError::codec(WEBP_ENCODE_FAILURE, error),
        tola_webp::WriteError::Write(error) => ImageError::codec(ENCODE_FAILURE, error),
    })
}

fn encode_png(
    writer: &mut impl Write,
    pixels: &ResampledPixels,
    cancellation: &dyn Cancellation,
) -> ImageResult<()> {
    let geometry = pixels.pixels();
    let width = geometry.width();
    let (color, mut rgb) = if pixels.has_transparency() {
        (png::ColorType::Rgba, Vec::new())
    } else {
        (png::ColorType::Rgb, zeroed_plane(width as usize * 3)?)
    };
    let output = PngOutput {
        writer,
        failure: None,
    };
    let mut encoder = png::Encoder::new(output, width, geometry.height());
    encoder.set_color(color);
    // Level 6 is the measured trade: level 9 saved at most 5% and sometimes grew the file,
    // for 1.4–3× the encode time.
    encoder.set_deflate_compression(png::DeflateCompression::Level(6));
    encoder.set_filter(png::Filter::Adaptive);
    let png_error = |error| ImageError::codec(ENCODE_FAILURE, error);
    let mut writer = encoder.write_header().map_err(png_error)?;
    let mut stream = writer
        .stream_writer_with_size(PNG_IDAT_BYTES)
        .map_err(png_error)?;
    for row in pixels.rgba().chunks_exact(width as usize * 4) {
        cancellation.ensure_active()?;
        let samples = if color == png::ColorType::Rgb {
            for (pixel, target) in row
                .as_chunks::<4>()
                .0
                .iter()
                .zip(rgb.as_chunks_mut::<3>().0)
            {
                target.copy_from_slice(&pixel[..3]);
            }
            rgb.as_slice()
        } else {
            row
        };
        stream
            .write_all(samples)
            .map_err(|error| ImageError::codec(ENCODE_FAILURE, error))?;
    }
    stream.finish().map_err(png_error)?;
    writer.finish().map_err(png_error)
}

/// Encode one plane as JPEG with half-resolution colour: 4:2:0.
///
/// Chroma subsampling is the size/quality trade photographs are made for; coloured text and fine
/// graphics belong in PNG or WebP.
fn encode_jpeg(
    writer: &mut impl Write,
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> ImageResult<()> {
    let too_wide = |actual| ImageError::TooLarge {
        limit: SizeLimit::FrameAxis,
        allowed: u64::from(u16::MAX),
        actual,
    };
    let width = u16::try_from(width).map_err(|_| too_wide(u64::from(width)))?;
    let height = u16::try_from(height).map_err(|_| too_wide(u64::from(height)))?;
    let mut encoder = jpeg_encoder::Encoder::new(writer, quality);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
    encoder
        .encode(pixels, width, height, jpeg_encoder::ColorType::Rgba)
        .map_err(|error| ImageError::codec(ENCODE_FAILURE, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::codec::tests::test_support::*;
    use crate::codec::{DecodedSource, inspect, render};
    use crate::{ImageFormat, OutputFormat, ResizeOperation, ResizeOptions};
    use std::io::Cursor;

    #[test]
    fn cancellation_precedes_encoding_mismatch() {
        let source = png(
            &[10, 20, 30, 255],
            1,
            1,
            ExtendedColorType::Rgba8,
            None,
            None,
        );
        let matching = recipe(&source, 1, 1, OutputFormat::Png, None);
        let mismatched = recipe(&source, 2, 1, OutputFormat::Png, None);
        let decoded = DecodedSource::open(&source, &NO_CANCELLATION).unwrap();
        let pixels = decoded.pixels(matching.pixels(), &NO_CANCELLATION).unwrap();
        let cancellation = || true;
        assert!(matches!(
            pixels.encode(&mismatched, &cancellation).unwrap_err(),
            crate::ImageError::Cancelled(_)
        ));
    }

    #[test]
    fn lossless_formats_reproduce_their_pixels() {
        let pixels = [200, 40, 70, 0, 10, 20, 30, 127, 99, 88, 77, 255];
        let source = png(&pixels, 3, 1, ExtendedColorType::Rgba8, None, None);
        for format in [OutputFormat::Png, OutputFormat::WebP] {
            let recipe = recipe(&source, 3, 1, format, None);
            let first = render(&source, &recipe, &NO_CANCELLATION).unwrap();
            let second = render(&source, &recipe, &NO_CANCELLATION).unwrap();
            assert_eq!(first, second);
            assert_eq!(rgba(&first), pixels);
        }
    }

    /// Lossy WebP encodes alpha without loss, so only its colour may move.
    #[test]
    fn lossy_webp_preserves_alpha() {
        let pixels = [200, 40, 70, 0, 10, 20, 30, 127, 99, 88, 77, 255];
        let source = png(&pixels, 3, 1, ExtendedColorType::Rgba8, None, None);
        let lossy = recipe(&source, 3, 1, OutputFormat::WebP, Some(0));
        let bytes = render(&source, &lossy, &NO_CANCELLATION).unwrap();
        assert!(inspect(&bytes).unwrap().is_lossy);
        assert_eq!(
            rgba(&bytes)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[3])
                .collect::<Vec<_>>(),
            [0, 127, 255]
        );
    }

    /// The quality knob is the trade it is documented to be: a lower quality publishes fewer
    /// bytes that stand further from the source. Either half alone would pass while the knob did
    /// nothing, so one case proves both.
    #[test]
    fn jpeg_quality_trades_size_for_fidelity() {
        let side = 64u32;
        let pixels: Vec<u8> = (0..side * side)
            .flat_map(|index| {
                let x = (index % side) as u8;
                let y = (index / side) as u8;
                [x.wrapping_mul(7), y.wrapping_mul(11), x ^ y]
            })
            .collect();
        // Opaque so JPEG may encode it at all.
        let source = png(&pixels, side, side, ExtendedColorType::Rgb8, None, None);
        let original = rgba(&source);
        let mut sizes = Vec::new();
        let mut deviations = Vec::new();
        for quality in [10u8, 90] {
            let recipe = recipe(&source, side, side, OutputFormat::Jpeg, Some(quality));
            let encoded = render(&source, &recipe, &NO_CANCELLATION).unwrap();
            sizes.push(encoded.len());
            let decoded = image::DynamicImage::from_decoder(
                image::codecs::jpeg::JpegDecoder::new(Cursor::new(&encoded[..])).unwrap(),
            )
            .unwrap()
            .to_rgb8();
            let deviation: u64 = original
                .as_chunks::<4>()
                .0
                .iter()
                .zip(decoded.pixels())
                .map(|(source, decoded)| {
                    u64::from(source[0].abs_diff(decoded[0]))
                        + u64::from(source[1].abs_diff(decoded[1]))
                        + u64::from(source[2].abs_diff(decoded[2]))
                })
                .sum();
            deviations.push(deviation);
        }
        assert!(
            sizes[0] < sizes[1],
            "quality 10 published {} bytes and quality 90 published {}",
            sizes[0],
            sizes[1]
        );
        assert!(
            deviations[0] > deviations[1],
            "quality 10 deviated by {} and quality 90 by {}",
            deviations[0],
            deviations[1]
        );
    }

    /// JPEG spends its bytes on luma: colour is subsampled at half resolution, 4:2:0.
    #[test]
    fn jpeg_subsamples_chroma() {
        let side = 16u32;
        let pixels: Vec<u8> = (0..side * side)
            .flat_map(|index| {
                let x = (index % side) as u8;
                let y = (index / side) as u8;
                [x.wrapping_mul(7), y.wrapping_mul(11), x ^ y]
            })
            .collect();
        let source = png(&pixels, side, side, ExtendedColorType::Rgb8, None, None);
        let encoded = render(
            &source,
            &recipe(&source, side, side, OutputFormat::Jpeg, Some(80)),
            &NO_CANCELLATION,
        )
        .unwrap();
        assert_eq!(jpeg_sampling_factors(&encoded), [0x22, 0x11, 0x11]);
    }

    /// The sampling factors of a baseline JPEG's three components, from its frame header.
    fn jpeg_sampling_factors(jpeg: &[u8]) -> [u8; 3] {
        let mut offset = 2;
        while offset + 4 <= jpeg.len() {
            assert_eq!(jpeg[offset], 0xFF, "every segment starts with a marker");
            if jpeg[offset + 1] == 0xC0 {
                assert_eq!(jpeg[offset + 9], 3, "a colour JPEG has three components");
                return [jpeg[offset + 11], jpeg[offset + 14], jpeg[offset + 17]];
            }
            let length = usize::from(u16::from_be_bytes([jpeg[offset + 2], jpeg[offset + 3]]));
            offset += 2 + length;
        }
        panic!("the JPEG carries no frame header");
    }

    /// Every encoder this crate writes is read back by its own scanner, and an unstated format
    /// follows what that scanner reports. A lossy source without transparency is re-encoded as
    /// JPEG; anything else stays lossless, so losing detail or transparency takes an explicit
    /// request.
    #[test]
    fn unstated_format_follows_the_encoder() {
        let samples = |channels: usize| -> Vec<u8> {
            (0..8 * 8)
                .flat_map(|index| {
                    let x = (index % 8) as u8;
                    let y = (index / 8) as u8;
                    let color = [
                        x.wrapping_mul(23),
                        y.wrapping_mul(37),
                        x ^ y,
                        y.wrapping_mul(9),
                    ];
                    color[..channels].to_vec()
                })
                .collect()
        };
        let opaque = png(&samples(3), 8, 8, ExtendedColorType::Rgb8, None, None);
        let transparent = png(&samples(4), 8, 8, ExtendedColorType::Rgba8, None, None);
        for (source, format, quality, lossy, resolved) in [
            (&opaque, OutputFormat::Png, None, false, ImageFormat::Png),
            (
                &transparent,
                OutputFormat::Png,
                None,
                false,
                ImageFormat::Png,
            ),
            (&opaque, OutputFormat::WebP, None, false, ImageFormat::Png),
            (
                &transparent,
                OutputFormat::WebP,
                None,
                false,
                ImageFormat::Png,
            ),
            (
                &opaque,
                OutputFormat::WebP,
                Some(80),
                true,
                ImageFormat::Jpeg,
            ),
            (
                &transparent,
                OutputFormat::WebP,
                Some(80),
                true,
                ImageFormat::Png,
            ),
            (
                &opaque,
                OutputFormat::Jpeg,
                Some(80),
                true,
                ImageFormat::Jpeg,
            ),
        ] {
            let encoded = render(
                source,
                &recipe(source, 8, 8, format, quality),
                &NO_CANCELLATION,
            )
            .unwrap();
            let metadata = inspect(&encoded).unwrap();
            assert_eq!(
                metadata.is_lossy, lossy,
                "{format:?} at {quality:?} reports its own lossiness"
            );
            let settled = ImageRecipe::resolve(
                &metadata,
                ResizeOptions {
                    width: Some(4),
                    height: Some(4),
                    operation: ResizeOperation::Fit,
                    ..ResizeOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                settled.format(),
                resolved,
                "an unstated format follows {format:?} at {quality:?}"
            );
        }
    }

    #[test]
    fn opaque_pixels_survive_lossless_encoding() {
        let pixels: Vec<u8> = (0..12)
            .flat_map(|value| [value * 7, value * 3, value * 11, u8::MAX])
            .collect();
        let source = png(&pixels, 4, 3, ExtendedColorType::Rgba8, None, None);
        let expected = rgba(&source);
        for format in [OutputFormat::Png, OutputFormat::WebP] {
            let recipe = recipe(&source, 4, 3, format, None);
            let encoded = render(&source, &recipe, &NO_CANCELLATION).unwrap();
            assert_eq!(rgba(&encoded), expected, "{format:?} changed opaque pixels");
        }
    }

    #[test]
    fn png_reports_failure_at_the_last_idat() {
        struct FailOnce(Option<usize>);

        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                match self.0 {
                    Some(0) => {
                        self.0 = None;
                        Err(io::ErrorKind::BrokenPipe.into())
                    }
                    Some(remaining) => {
                        let accepted = bytes.len().min(remaining);
                        self.0 = Some(remaining - accepted);
                        Ok(accepted)
                    }
                    None => Ok(bytes.len()),
                }
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let source = png(&[10, 20, 30], 1, 1, ExtendedColorType::Rgb8, None, None);
        let recipe = recipe(&source, 1, 1, OutputFormat::Png, None);
        let pixels = DecodedSource::open(&source, &NO_CANCELLATION)
            .unwrap()
            .into_pixels(recipe.pixels(), &NO_CANCELLATION)
            .unwrap();
        let encoded = pixels.encode(&recipe, &NO_CANCELLATION).unwrap();
        let mut offset = 8;
        let mut last_idat_crc = None;
        while offset < encoded.len() {
            let len = u32::from_be_bytes(encoded[offset..][..4].try_into().unwrap()) as usize;
            let crc = offset + 8 + len;
            if &encoded[offset + 4..][..4] == b"IDAT" {
                last_idat_crc = Some(crc);
            }
            offset = crc + 4;
        }

        let mut destination = FailOnce(last_idat_crc);
        assert!(
            pixels
                .encode_to(&mut destination, &recipe, &NO_CANCELLATION)
                .is_err()
        );
        assert!(destination.0.is_none());
    }

    #[test]
    fn one_resample_serves_every_encoding() {
        let pixels: Vec<u8> = (0..16)
            .flat_map(|value| [value * 5, value, value * 3, u8::MAX])
            .collect();
        let source = png(&pixels, 4, 4, ExtendedColorType::Rgba8, None, None);
        let metadata = inspect(&source).unwrap();
        let resolve = |width: u32, height: u32, format: OutputFormat, quality: Option<u8>| {
            ImageRecipe::resolve(
                &metadata,
                ResizeOptions {
                    width: Some(width),
                    height: Some(height),
                    operation: ResizeOperation::Scale,
                    format,
                    quality,
                    ..ResizeOptions::default()
                },
            )
            .unwrap()
        };
        let recipes = [
            resolve(2, 2, OutputFormat::Png, None),
            resolve(2, 2, OutputFormat::WebP, Some(80)),
            resolve(2, 2, OutputFormat::WebP, None),
        ];
        let alone: Vec<_> = recipes
            .iter()
            .map(|recipe| render(&source, recipe, &NO_CANCELLATION).unwrap())
            .collect();

        let decoded = DecodedSource::open(&source, &NO_CANCELLATION).unwrap();
        let resampled = decoded
            .pixels(recipes[0].pixels(), &NO_CANCELLATION)
            .unwrap();
        for (recipe, expected) in recipes.iter().zip(&alone) {
            assert_eq!(
                &resampled.encode(recipe, &NO_CANCELLATION).unwrap(),
                expected
            );
        }
        assert!(
            resampled
                .encode(&resolve(3, 2, OutputFormat::Png, None), &NO_CANCELLATION)
                .is_err()
        );
        // A length check alone cannot refuse the same pixels in another shape: 2x2 and 4x1 hold
        // the same count, so only the pixels' own geometry can refuse a derivative that would be
        // encoded from a mismatch.
        let reshaped = resolve(4, 1, OutputFormat::Png, None);
        let (resampled_pixels, reshaped_pixels) = (resampled.pixels(), reshaped.pixels());
        assert_eq!(
            resampled_pixels.width() * resampled_pixels.height(),
            reshaped_pixels.width() * reshaped_pixels.height()
        );
        assert!(resampled.encode(&reshaped, &NO_CANCELLATION).is_err());
    }
}
