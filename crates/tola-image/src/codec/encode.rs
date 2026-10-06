//! Encoding resampled pixels into the format a recipe asks for.
use std::io::{self, Write};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

use crate::cancellation::Cancellation;
use crate::recipe::Encoding;
use crate::resample::ResampledPixels;
use crate::rows::for_each_row_pair;
use crate::{ImageRecipe, pixel_bytes};

struct EncodeWriter<'a> {
    bytes: Vec<u8>,
    cancellation: &'a dyn Cancellation,
}

impl Write for EncodeWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancellation
            .ensure_active()
            .map_err(io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.cancellation.ensure_active().map_err(io::Error::other)
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
    ) -> Result<Arc<[u8]>> {
        cancellation.ensure_active()?;
        ensure!(
            recipe.pixels() == self.pixels(),
            "the image could not be encoded at this size; try another `width` or `height`"
        );
        let result = encode(self.rgba(), self.has_transparency(), recipe, cancellation);
        cancellation.ensure_active()?;
        result
    }
}

fn encode(
    rgba: &[u8],
    has_transparency: bool,
    recipe: &ImageRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Arc<[u8]>> {
    if let Encoding::WebP { quality } = recipe.encoding {
        let encoder = webp::Encoder::from_rgba(rgba, recipe.width(), recipe.height());
        let mut config = webp::WebPConfig::new().map_err(|_| {
            anyhow::anyhow!("Tola could not encode this image as WebP; use `format: \"png\"`")
        })?;
        config.lossless = i32::from(quality.is_none());
        config.quality = f32::from(quality.unwrap_or(75));
        config.method = 4;
        config.thread_level = 0;
        config.exact = 1;
        config.alpha_quality = 100;
        config.near_lossless = 100;
        cancellation.ensure_active()?;
        // The safe libwebp wrapper has no cancellation callback. Never publish its result if
        // cancellation arrived during this indivisible codec call.
        let encoded = encoder.encode_advanced(&config);
        cancellation.ensure_active()?;
        let encoded = encoded.map_err(|_| {
            anyhow::anyhow!("Tola could not encode this image as WebP; use `format: \"png\"`")
        })?;
        ensure!(
            !encoded.is_empty(),
            "Tola could not encode this image as WebP; use `format: \"png\"`"
        );
        return Ok(Arc::from(&encoded[..]));
    }
    let rgb;
    let (pixels, color) = if has_transparency {
        (rgba, ExtendedColorType::Rgba8)
    } else {
        rgb = rgb_plane(rgba, recipe.width(), recipe.height(), cancellation)?;
        (rgb.as_slice(), ExtendedColorType::Rgb8)
    };
    let mut writer = EncodeWriter {
        bytes: Vec::new(),
        cancellation,
    };
    let result = match recipe.encoding {
        Encoding::Jpeg { quality } => JpegEncoder::new_with_quality(&mut writer, quality)
            .write_image(pixels, recipe.width(), recipe.height(), color),
        Encoding::Png => PngEncoder::new_with_quality(
            &mut writer,
            CompressionType::Level(6),
            FilterType::Adaptive,
        )
        .write_image(pixels, recipe.width(), recipe.height(), color),
        Encoding::WebP { .. } => {
            bail!("Tola could not encode the resized image; try another `format`")
        }
    };
    // Restore typed cancellation instead of leaking the codec's wrapped I/O error.
    cancellation.ensure_active()?;
    result.context("Tola could not encode the resized image; try another `format`")?;
    Ok(writer.bytes.into())
}

/// The color samples of an opaque plane, without its alpha channel.
fn rgb_plane(
    rgba: &[u8],
    width: u32,
    height: u32,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let bytes = pixel_bytes(width, height, 3)?;
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(bytes)
        .context("the image is too large to hold in memory; use a smaller image")?;
    rgb.resize(bytes, 0);
    for_each_row_pair(
        &mut rgb,
        width as usize * 3,
        rgba,
        width as usize * 4,
        || (),
        |(), _, source, row| {
            cancellation.ensure_active()?;
            for (pixel, colored) in source
                .as_chunks::<4>()
                .0
                .iter()
                .zip(row.as_chunks_mut::<3>().0.iter_mut())
            {
                colored.copy_from_slice(&pixel[..3]);
            }
            Ok(())
        },
    )?;
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::test_support::*;
    use crate::codec::{DecodedSource, inspect, render};
    use crate::{ImageFormat, OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions};
    use std::io::Cursor;

    #[test]
    fn opaque_webp_keeps_rgb_encoding_bytes() {
        for (width, height) in [(1u32, 1u32), (3, 7), (17, 13), (64, 65)] {
            for pattern in 0..3u32 {
                let pixels: Vec<u8> = (0..width * height)
                    .flat_map(|index| {
                        let value = match pattern {
                            0 => 79,
                            1 => index.wrapping_mul(2_654_435_761),
                            _ => (index % width) * 7 + (index / width) * 11,
                        };
                        [value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]
                    })
                    .collect();
                let source = png(&pixels, width, height, ExtendedColorType::Rgba8, None, None);
                let rgb: Vec<_> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|pixel| pixel[..3].iter().copied())
                    .collect();
                for quality in [None, Some(0), Some(75), Some(100)] {
                    let mut config = webp::WebPConfig::new().unwrap();
                    config.lossless = i32::from(quality.is_none());
                    config.quality = f32::from(quality.unwrap_or(75));
                    config.method = 4;
                    config.thread_level = 0;
                    config.exact = 1;
                    config.alpha_quality = 100;
                    config.near_lossless = 100;
                    let rgb_encoded = webp::Encoder::from_rgb(&rgb, width, height)
                        .encode_advanced(&config)
                        .unwrap();
                    let encoded = render(
                        &source,
                        &recipe(&source, width, height, OutputFormat::WebP, quality),
                        &active,
                    )
                    .unwrap();
                    assert_eq!(
                        &encoded[..],
                        &rgb_encoded[..],
                        "{width}x{height}, pattern {pattern}, quality {quality:?}"
                    );
                }
            }
        }
    }

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
        let decoded = DecodedSource::open(&source, &active).unwrap();
        let pixels = decoded.pixels(matching.pixels(), &active).unwrap();
        let cancellation = || true;
        assert!(
            pixels
                .encode(&mismatched, &cancellation)
                .unwrap_err()
                .is::<crate::ImageCancelled>()
        );
    }

    #[test]
    fn lossless_formats_reproduce_their_pixels() {
        let pixels = [200, 40, 70, 0, 10, 20, 30, 127, 99, 88, 77, 255];
        let source = png(&pixels, 3, 1, ExtendedColorType::Rgba8, None, None);
        for format in [OutputFormat::Png, OutputFormat::WebP] {
            let recipe = recipe(&source, 3, 1, format, None);
            let first = render(&source, &recipe, &active).unwrap();
            let second = render(&source, &recipe, &active).unwrap();
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
        let bytes = render(&source, &lossy, &active).unwrap();
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
            let encoded = render(&source, &recipe, &active).unwrap();
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
            let encoded = render(source, &recipe(source, 8, 8, format, quality), &active).unwrap();
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
                    format: OutputFormat::Auto,
                    quality: None,
                    filter: ResizeFilter::Lanczos3,
                    background: None,
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
            let encoded = render(&source, &recipe, &active).unwrap();
            assert_eq!(rgba(&encoded), expected, "{format:?} changed opaque pixels");
        }
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
                    filter: ResizeFilter::Lanczos3,
                    background: None,
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
            .map(|recipe| render(&source, recipe, &active).unwrap())
            .collect();

        let decoded = DecodedSource::open(&source, &active).unwrap();
        let resampled = decoded.pixels(recipes[0].pixels(), &active).unwrap();
        for (recipe, expected) in recipes.iter().zip(&alone) {
            assert_eq!(&resampled.encode(recipe, &active).unwrap(), expected);
        }
        assert!(
            resampled
                .encode(&resolve(3, 2, OutputFormat::Png, None), &active)
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
        assert!(resampled.encode(&reshaped, &active).is_err());
    }
}
