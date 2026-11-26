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
    dst: W,
    cancel: &'a dyn Cancellation,
}

impl<W: Write> Write for EncodeWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancel.ensure_active().map_err(io::Error::other)?;
        self.dst.write(bytes)
    }

    fn write_vectored(&mut self, chunks: &[IoSlice<'_>]) -> io::Result<usize> {
        self.cancel.ensure_active().map_err(io::Error::other)?;
        self.dst.write_vectored(chunks)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.cancel.ensure_active().map_err(io::Error::other)?;
        self.dst.flush()
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
        self.writer.write_all(bytes).inspect_err(|err| {
            self.failure = Some(err.kind());
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
        let px = u64::from(self.width()) * u64::from(self.height());
        let alpha = may_have_transparency && self.pixels().background.is_none();
        match self.encoding {
            // The writer and shared output can coexist while the output is published.
            Encoding::Jpeg { .. } => px.saturating_mul(2),
            Encoding::Png => {
                // The stream holds three scanlines. Opaque output also converts one RGB row.
                let rows = u64::from(self.width()) * 12;
                let ch = if alpha { 4 } else { 3 };
                px.saturating_mul(2 * ch)
                    .saturating_add(rows + PNG_IDAT_BYTES as u64)
            }
            // VP8L also keeps a mutable four-channel transform buffer.
            Encoding::WebP { quality: None } => px.saturating_mul(12),
            Encoding::WebP { quality: Some(_) } => {
                tola_webp::estimated_scratch_bytes(self.width(), self.height(), workers, alpha)
            }
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
        cancel: &dyn Cancellation,
    ) -> ImageResult<Arc<[u8]>> {
        self.encode_bytes(recipe, cancel, None)
    }

    /// Encode with a maximum worker count, including the calling thread.
    pub fn encode_with_workers(
        &self,
        recipe: &ImageRecipe,
        cancel: &dyn Cancellation,
        workers: usize,
    ) -> ImageResult<Arc<[u8]>> {
        self.encode_bytes(recipe, cancel, Some(workers))
    }

    fn encode_bytes(
        &self,
        recipe: &ImageRecipe,
        cancel: &dyn Cancellation,
        workers: Option<usize>,
    ) -> ImageResult<Arc<[u8]>> {
        let mut bytes = Vec::new();
        self.write_encoded(&mut bytes, recipe, cancel, workers)?;
        let encoded = bytes.into();
        cancel.ensure_active()?;
        Ok(encoded)
    }

    /// Encode to a caller-owned destination without retaining another encoded output here.
    /// Codecs can retain compressed chunks before writing the container.
    pub fn encode_to(
        &self,
        dst: impl Write,
        recipe: &ImageRecipe,
        cancel: &dyn Cancellation,
    ) -> ImageResult<ImageMetadata> {
        self.write_encoded(dst, recipe, cancel, None)
    }

    fn write_encoded(
        &self,
        dst: impl Write,
        recipe: &ImageRecipe,
        cancel: &dyn Cancellation,
        workers: Option<usize>,
    ) -> ImageResult<ImageMetadata> {
        cancel.ensure_active()?;
        if recipe.pixels() != self.pixels() {
            return Err(ImageError::unreadable(
                "the image could not be encoded at this size; try another `width` or `height`",
            ));
        }
        let mut writer = EncodeWriter { dst, cancel };
        let res = match recipe.encoding {
            Encoding::Jpeg { quality } if !self.has_transparency() => encode_jpeg(
                &mut writer,
                self.rgba(),
                recipe.width(),
                recipe.height(),
                quality,
            ),
            Encoding::Jpeg { .. } => Err(ImageError::unreadable(ENCODE_FAILURE)),
            Encoding::Png => encode_png(&mut writer, self, cancel),
            Encoding::WebP { quality } => encode_webp(&mut writer, self, quality, workers, cancel),
        };
        cancel.ensure_active()?;
        res?;
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
    px: &ResampledPixels,
    q: Option<u8>,
    workers: Option<usize>,
    cancel: &dyn Cancellation,
) -> ImageResult<()> {
    let geom = px.pixels();
    let Some(q) = q else {
        return image::codecs::webp::WebPEncoder::new_lossless(writer)
            .write_image(
                px.rgba(),
                geom.width(),
                geom.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|err| match err {
                image::ImageError::IoError(err) => ImageError::codec(ENCODE_FAILURE, err),
                err => ImageError::codec(WEBP_ENCODE_FAILURE, err),
            });
    };

    let mut opts = workers.map_or_else(
        || tola_webp::EncodeOptions::new(q),
        |workers| tola_webp::EncodeOptions::with_workers(q, workers),
    );
    opts.has_transparency = Some(px.has_transparency());
    tola_webp::encode_rgba_to(
        px.rgba(),
        geom.width(),
        geom.height(),
        opts,
        &|| cancel.is_cancelled(),
        writer,
    )
    .map_err(|err| match err {
        tola_webp::WriteError::Encode(tola_webp::Error::Cancelled) => crate::ImageCancelled.into(),
        tola_webp::WriteError::Encode(err) => ImageError::codec(WEBP_ENCODE_FAILURE, err),
        tola_webp::WriteError::Write(err) => ImageError::codec(ENCODE_FAILURE, err),
    })
}

fn encode_png(
    writer: &mut impl Write,
    px: &ResampledPixels,
    cancel: &dyn Cancellation,
) -> ImageResult<()> {
    let geom = px.pixels();
    let w = geom.width();
    let (color, mut rgb) = if px.has_transparency() {
        (png::ColorType::Rgba, Vec::new())
    } else {
        (png::ColorType::Rgb, zeroed_plane(w as usize * 3)?)
    };
    let out = PngOutput {
        writer,
        failure: None,
    };
    let mut enc = png::Encoder::new(out, w, geom.height());
    enc.set_color(color);
    // Level 6 is the measured trade: level 9 saved at most 5% and sometimes grew the file,
    // for 1.4–3× the encode time.
    enc.set_deflate_compression(png::DeflateCompression::Level(6));
    enc.set_filter(png::Filter::Adaptive);
    let png_err = |err| ImageError::codec(ENCODE_FAILURE, err);
    let mut writer = enc.write_header().map_err(png_err)?;
    let mut stream = writer
        .stream_writer_with_size(PNG_IDAT_BYTES)
        .map_err(png_err)?;
    for row in px.rgba().chunks_exact(w as usize * 4) {
        cancel.ensure_active()?;
        let samples = if color == png::ColorType::Rgb {
            for (p, dst) in row
                .as_chunks::<4>()
                .0
                .iter()
                .zip(rgb.as_chunks_mut::<3>().0)
            {
                dst.copy_from_slice(&p[..3]);
            }
            rgb.as_slice()
        } else {
            row
        };
        stream
            .write_all(samples)
            .map_err(|err| ImageError::codec(ENCODE_FAILURE, err))?;
    }
    stream.finish().map_err(png_err)?;
    writer.finish().map_err(png_err)
}

/// Encode one plane as JPEG with half-resolution colour: 4:2:0.
fn encode_jpeg(writer: &mut impl Write, px: &[u8], w: u32, h: u32, q: u8) -> ImageResult<()> {
    let too_wide = |actual| ImageError::TooLarge {
        limit: SizeLimit::FrameAxis,
        allowed: u64::from(u16::MAX),
        actual,
    };
    let w = u16::try_from(w).map_err(|_| too_wide(u64::from(w)))?;
    let h = u16::try_from(h).map_err(|_| too_wide(u64::from(h)))?;
    let mut enc = jpeg_encoder::Encoder::new(writer, q);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
    enc.encode(px, w, h, jpeg_encoder::ColorType::Rgba)
        .map_err(|err| ImageError::codec(ENCODE_FAILURE, err))
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
    fn cancel_before_mismatch() {
        let src = png(
            &[10, 20, 30, 255],
            1,
            1,
            ExtendedColorType::Rgba8,
            None,
            None,
        );
        let matching = recipe(&src, 1, 1, OutputFormat::Png, None);
        let mismatched = recipe(&src, 2, 1, OutputFormat::Png, None);
        let decoded = DecodedSource::open(&src, &NO_CANCELLATION).unwrap();
        let px = decoded.pixels(matching.pixels(), &NO_CANCELLATION).unwrap();
        let cancel = || true;
        assert!(matches!(
            px.encode(&mismatched, &cancel).unwrap_err(),
            crate::ImageError::Cancelled(_)
        ));
    }

    #[test]
    fn lossless_roundtrip_bytes() {
        let px = [200, 40, 70, 0, 10, 20, 30, 127, 99, 88, 77, 255];
        let src = png(&px, 3, 1, ExtendedColorType::Rgba8, None, None);
        for fmt in [OutputFormat::Png, OutputFormat::WebP] {
            let r = recipe(&src, 3, 1, fmt, None);
            let a = render(&src, &r, &NO_CANCELLATION).unwrap();
            let b = render(&src, &r, &NO_CANCELLATION).unwrap();
            assert_eq!(a, b);
            assert_eq!(rgba(&a), px);
        }
    }

    #[test]
    fn lossy_webp_keeps_alpha() {
        let px = [200, 40, 70, 0, 10, 20, 30, 127, 99, 88, 77, 255];
        let src = png(&px, 3, 1, ExtendedColorType::Rgba8, None, None);
        let lossy = recipe(&src, 3, 1, OutputFormat::WebP, Some(0));
        let bytes = render(&src, &lossy, &NO_CANCELLATION).unwrap();
        assert!(inspect(&bytes).unwrap().is_lossy);
        assert_eq!(
            rgba(&bytes)
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[3])
                .collect::<Vec<_>>(),
            [0, 127, 255]
        );
    }

    #[test]
    fn jpeg_quality_size_fidelity() {
        let side = 64u32;
        let px: Vec<u8> = (0..side * side)
            .flat_map(|i| {
                let x = (i % side) as u8;
                let y = (i / side) as u8;
                [x.wrapping_mul(7), y.wrapping_mul(11), x ^ y]
            })
            .collect();
        let src = png(&px, side, side, ExtendedColorType::Rgb8, None, None);
        let orig = rgba(&src);
        let mut sizes = Vec::new();
        let mut devs = Vec::new();
        for q in [10u8, 90] {
            let r = recipe(&src, side, side, OutputFormat::Jpeg, Some(q));
            let enc = render(&src, &r, &NO_CANCELLATION).unwrap();
            sizes.push(enc.len());
            let dec = image::DynamicImage::from_decoder(
                image::codecs::jpeg::JpegDecoder::new(Cursor::new(&enc[..])).unwrap(),
            )
            .unwrap()
            .to_rgb8();
            let dev: u64 = orig
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dec.pixels())
                .map(|(s, d)| {
                    u64::from(s[0].abs_diff(d[0]))
                        + u64::from(s[1].abs_diff(d[1]))
                        + u64::from(s[2].abs_diff(d[2]))
                })
                .sum();
            devs.push(dev);
        }
        assert!(sizes[0] < sizes[1], "{} vs {}", sizes[0], sizes[1]);
        assert!(devs[0] > devs[1], "{} vs {}", devs[0], devs[1]);
    }

    #[test]
    fn jpeg_subsamples_420() {
        let side = 16u32;
        let px: Vec<u8> = (0..side * side)
            .flat_map(|i| {
                let x = (i % side) as u8;
                let y = (i / side) as u8;
                [x.wrapping_mul(7), y.wrapping_mul(11), x ^ y]
            })
            .collect();
        let src = png(&px, side, side, ExtendedColorType::Rgb8, None, None);
        let enc = render(
            &src,
            &recipe(&src, side, side, OutputFormat::Jpeg, Some(80)),
            &NO_CANCELLATION,
        )
        .unwrap();
        assert_eq!(jpeg_sampling_factors(&enc), [0x22, 0x11, 0x11]);
    }

    fn jpeg_sampling_factors(jpeg: &[u8]) -> [u8; 3] {
        let mut off = 2;
        while off + 4 <= jpeg.len() {
            assert_eq!(jpeg[off], 0xFF, "every segment starts with a marker");
            if jpeg[off + 1] == 0xC0 {
                assert_eq!(jpeg[off + 9], 3, "a colour JPEG has three components");
                return [jpeg[off + 11], jpeg[off + 14], jpeg[off + 17]];
            }
            let len = usize::from(u16::from_be_bytes([jpeg[off + 2], jpeg[off + 3]]));
            off += 2 + len;
        }
        panic!("the JPEG carries no frame header");
    }

    #[test]
    fn auto_format_follows_encoder() {
        let samples = |ch: usize| -> Vec<u8> {
            (0..8 * 8)
                .flat_map(|i| {
                    let x = (i % 8) as u8;
                    let y = (i / 8) as u8;
                    let c = [
                        x.wrapping_mul(23),
                        y.wrapping_mul(37),
                        x ^ y,
                        y.wrapping_mul(9),
                    ];
                    c[..ch].to_vec()
                })
                .collect()
        };
        let opaque = png(&samples(3), 8, 8, ExtendedColorType::Rgb8, None, None);
        let alpha = png(&samples(4), 8, 8, ExtendedColorType::Rgba8, None, None);
        for (src, fmt, q, lossy, resolved) in [
            (&opaque, OutputFormat::Png, None, false, ImageFormat::Png),
            (&alpha, OutputFormat::Png, None, false, ImageFormat::Png),
            (&opaque, OutputFormat::WebP, None, false, ImageFormat::Png),
            (&alpha, OutputFormat::WebP, None, false, ImageFormat::Png),
            (
                &opaque,
                OutputFormat::WebP,
                Some(80),
                true,
                ImageFormat::Jpeg,
            ),
            (&alpha, OutputFormat::WebP, Some(80), true, ImageFormat::Png),
            (
                &opaque,
                OutputFormat::Jpeg,
                Some(80),
                true,
                ImageFormat::Jpeg,
            ),
        ] {
            let enc = render(src, &recipe(src, 8, 8, fmt, q), &NO_CANCELLATION).unwrap();
            let meta = inspect(&enc).unwrap();
            assert_eq!(meta.is_lossy, lossy, "{fmt:?} at {q:?}");
            let settled = ImageRecipe::resolve(
                &meta,
                ResizeOptions {
                    width: Some(4),
                    height: Some(4),
                    operation: ResizeOperation::Fit,
                    ..ResizeOptions::default()
                },
            )
            .unwrap();
            assert_eq!(settled.format(), resolved, "{fmt:?} at {q:?}");
        }
    }

    #[test]
    fn opaque_lossless_roundtrip() {
        let px: Vec<u8> = (0..12)
            .flat_map(|v| [v * 7, v * 3, v * 11, u8::MAX])
            .collect();
        let src = png(&px, 4, 3, ExtendedColorType::Rgba8, None, None);
        let expected = rgba(&src);
        for fmt in [OutputFormat::Png, OutputFormat::WebP] {
            let r = recipe(&src, 4, 3, fmt, None);
            let enc = render(&src, &r, &NO_CANCELLATION).unwrap();
            assert_eq!(rgba(&enc), expected, "{fmt:?} changed opaque pixels");
        }
    }

    #[test]
    fn png_last_idat_error() {
        struct FailOnce(Option<usize>);

        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                match self.0 {
                    Some(0) => {
                        self.0 = None;
                        Err(io::ErrorKind::BrokenPipe.into())
                    }
                    Some(rem) => {
                        let n = bytes.len().min(rem);
                        self.0 = Some(rem - n);
                        Ok(n)
                    }
                    None => Ok(bytes.len()),
                }
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let src = png(&[10, 20, 30], 1, 1, ExtendedColorType::Rgb8, None, None);
        let r = recipe(&src, 1, 1, OutputFormat::Png, None);
        let px = DecodedSource::open(&src, &NO_CANCELLATION)
            .unwrap()
            .into_pixels(r.pixels(), &NO_CANCELLATION)
            .unwrap();
        let enc = px.encode(&r, &NO_CANCELLATION).unwrap();
        let mut off = 8;
        let mut last_idat_crc = None;
        while off < enc.len() {
            let len = u32::from_be_bytes(enc[off..][..4].try_into().unwrap()) as usize;
            let crc = off + 8 + len;
            if &enc[off + 4..][..4] == b"IDAT" {
                last_idat_crc = Some(crc);
            }
            off = crc + 4;
        }

        let mut dst = FailOnce(last_idat_crc);
        assert!(px.encode_to(&mut dst, &r, &NO_CANCELLATION).is_err());
        assert!(dst.0.is_none());
    }

    #[test]
    fn shared_resample_multi_encode() {
        let px: Vec<u8> = (0..16).flat_map(|v| [v * 5, v, v * 3, u8::MAX]).collect();
        let src = png(&px, 4, 4, ExtendedColorType::Rgba8, None, None);
        let meta = inspect(&src).unwrap();
        let resolve = |w: u32, h: u32, format: OutputFormat, quality: Option<u8>| {
            ImageRecipe::resolve(
                &meta,
                ResizeOptions {
                    width: Some(w),
                    height: Some(h),
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
            .map(|r| render(&src, r, &NO_CANCELLATION).unwrap())
            .collect();

        let decoded = DecodedSource::open(&src, &NO_CANCELLATION).unwrap();
        let resampled = decoded
            .pixels(recipes[0].pixels(), &NO_CANCELLATION)
            .unwrap();
        for (r, expected) in recipes.iter().zip(&alone) {
            assert_eq!(&resampled.encode(r, &NO_CANCELLATION).unwrap(), expected);
        }
        assert!(
            resampled
                .encode(&resolve(3, 2, OutputFormat::Png, None), &NO_CANCELLATION)
                .is_err()
        );
        // 2x2 and 4x1 have equal pixel counts, so shape mismatch must still fail.
        let reshaped = resolve(4, 1, OutputFormat::Png, None);
        let (a, b) = (resampled.pixels(), reshaped.pixels());
        assert_eq!(a.width() * a.height(), b.width() * b.height());
        assert!(resampled.encode(&reshaped, &NO_CANCELLATION).is_err());
    }
}
