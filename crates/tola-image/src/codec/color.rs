//! Normalizing decoded planes to the sRGB RGBA8 pixels every recipe starts from.

use std::borrow::Cow;

use anyhow::{Context, Result, bail, ensure};
use image::ColorType;
use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};

use crate::cancellation::Cancellation;
use crate::pixel_bytes;
use crate::rows::rewrite_rows_in_place;

/// How a decode converts its samples to sRGB.
pub(in crate::codec) enum ColorTransform<'a> {
    /// Convert through the ICC profile.
    Icc(Cow<'a, [u8]>),
    /// Decode through the gamma exponent.
    Gamma(f32),
}

pub(in crate::codec) fn normalize_color(
    raw: Vec<u8>,
    color: ColorType,
    dimensions: (u32, u32),
    transform: Option<ColorTransform<'_>>,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let (width, _) = dimensions;
    cancellation.ensure_active()?;
    if color == ColorType::Rgba8 && transform.is_none() {
        return Ok(raw);
    }
    let (layout, is_sixteen_bit) = match color {
        ColorType::L8 => (Layout::Gray, false),
        ColorType::La8 => (Layout::GrayAlpha, false),
        ColorType::Rgb8 => (Layout::Rgb, false),
        ColorType::Rgba8 => (Layout::Rgba, false),
        ColorType::L16 => (Layout::Gray, true),
        ColorType::La16 => (Layout::GrayAlpha, true),
        ColorType::Rgb16 => (Layout::Rgb, true),
        ColorType::Rgba16 => (Layout::Rgba, true),
        _ => bail!(
            "this image uses a color format Tola cannot resize; save it as 8-bit or 16-bit RGB"
        ),
    };
    let grayscale = matches!(layout, Layout::Gray | Layout::GrayAlpha);
    let profile = match transform {
        Some(ColorTransform::Icc(icc)) => {
            let profile = ColorProfile::new_from_slice(icc.as_ref())
                .context("the embedded color profile is damaged; convert the image to sRGB")?;
            ensure!(
                profile.color_space
                    == if grayscale {
                        DataColorSpace::Gray
                    } else {
                        DataColorSpace::Rgb
                    },
                "the embedded color profile does not match the image's color space; convert the image to sRGB",
            );
            Some(profile)
        }
        Some(ColorTransform::Gamma(gamma)) => {
            if grayscale {
                Some(ColorProfile::new_gray_with_gamma(gamma))
            } else {
                let mut profile = ColorProfile::new_srgb();
                profile.cicp = None;
                let curve = moxcms::curve_from_gamma(gamma);
                profile.red_trc = Some(curve.clone());
                profile.green_trc = Some(curve.clone());
                profile.blue_trc = Some(curve);
                Some(profile)
            }
        }
        None => None,
    };
    cancellation.ensure_active()?;
    let Some(profile) = profile else {
        let sample_bytes = if is_sixteen_bit { 2 } else { 1 };
        return rewrite_plane(
            raw,
            layout,
            sample_bytes,
            dimensions,
            cancellation,
            || (),
            |(), input, output| {
                cancellation.ensure_active()?;
                for (src, dst) in input
                    .chunks_exact(layout.channels() * sample_bytes)
                    .zip(output.as_chunks_mut::<4>().0.iter_mut())
                {
                    let sample = |channel: usize| -> u8 {
                        if is_sixteen_bit {
                            let offset = channel * 2;
                            ((u32::from(u16::from_ne_bytes([src[offset], src[offset + 1]])) + 128)
                                / 257) as u8
                        } else {
                            src[channel]
                        }
                    };
                    dst[0] = sample(0);
                    dst[1] = sample(if grayscale { 0 } else { 1 });
                    dst[2] = sample(if grayscale { 0 } else { 2 });
                    dst[3] = if layout.has_alpha() {
                        sample(layout.channels() - 1)
                    } else {
                        u8::MAX
                    };
                }
                Ok(())
            },
        );
    };
    static SRGB: std::sync::LazyLock<ColorProfile> =
        std::sync::LazyLock::new(ColorProfile::new_srgb);
    let srgb = &*SRGB;
    // Alpha is coverage, not a color channel. Transform only RGB/gray
    // samples and restore source alpha independently for both precisions.
    let color_layout = if grayscale { Layout::Gray } else { Layout::Rgb };
    let color_channels = color_layout.channels();
    let options = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    if is_sixteen_bit {
        let transform = profile
            .create_transform_16bit(color_layout, srgb, Layout::Rgba, options)
            .context(
                "the embedded color profile cannot be applied to this image; convert the image to sRGB",
            )?;
        cancellation.ensure_active()?;
        rewrite_plane(
            raw,
            layout,
            2,
            dimensions,
            cancellation,
            || {
                (
                    vec![0u16; width as usize * color_channels],
                    vec![0u16; width as usize * 4],
                )
            },
            |(input_row, output_row), input, output| {
                cancellation.ensure_active()?;
                for (source, destination) in input
                    .chunks_exact(layout.channels() * 2)
                    .zip(input_row.chunks_exact_mut(color_channels))
                {
                    for (sample, bytes) in
                        destination.iter_mut().zip(source.as_chunks::<2>().0.iter())
                    {
                        *sample = u16::from_ne_bytes([bytes[0], bytes[1]]);
                    }
                }
                transform.transform(input_row, output_row).context(
                    "the embedded color profile cannot be applied to this image; convert the image to sRGB",
                )?;
                for (dst, src) in output.iter_mut().zip(output_row.iter()) {
                    *dst = ((u32::from(*src) + 128) / 257) as u8;
                }
                if layout.has_alpha() {
                    for (source, destination) in input
                        .chunks_exact(layout.channels() * 2)
                        .zip(output.as_chunks_mut::<4>().0.iter_mut())
                    {
                        let offset = (layout.channels() - 1) * 2;
                        let alpha = u16::from_ne_bytes([source[offset], source[offset + 1]]);
                        destination[3] = ((u32::from(alpha) + 128) / 257) as u8;
                    }
                }
                Ok(())
            },
        )
    } else {
        let transform = profile
            .create_transform_8bit(color_layout, srgb, Layout::Rgba, options)
            .context(
                "the embedded color profile cannot be applied to this image; convert the image to sRGB",
            )?;
        cancellation.ensure_active()?;
        rewrite_plane(
            raw,
            layout,
            1,
            dimensions,
            cancellation,
            || {
                if layout.has_alpha() {
                    vec![0; width as usize * color_channels]
                } else {
                    Vec::new()
                }
            },
            |input_row, input, output| {
                cancellation.ensure_active()?;
                let color_input = if layout.has_alpha() {
                    for (source, destination) in input
                        .chunks_exact(layout.channels())
                        .zip(input_row.chunks_exact_mut(color_channels))
                    {
                        destination.copy_from_slice(&source[..color_channels]);
                    }
                    input_row.as_slice()
                } else {
                    input
                };
                transform.transform(color_input, output).context(
                    "the embedded color profile cannot be applied to this image; convert the image to sRGB",
                )?;
                if layout.has_alpha() {
                    for (source, destination) in input
                        .chunks_exact(layout.channels())
                        .zip(output.as_chunks_mut::<4>().0.iter_mut())
                    {
                        destination[3] = source[layout.channels() - 1];
                    }
                }
                Ok(())
            },
        )
    }
}

/// Convert one decoded plane to the RGBA8 pixels a recipe starts from, in place.
///
/// The plane is grown or shrunk to four bytes per pixel and its rows are rewritten where they lie,
/// so a decode holds one plane rather than the decoded plane and its normalized copy at once.
fn rewrite_plane<S, I, F>(
    raw: Vec<u8>,
    layout: Layout,
    sample_bytes: usize,
    dimensions: (u32, u32),
    cancellation: &dyn Cancellation,
    initial: I,
    row_work: F,
) -> Result<Vec<u8>>
where
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, &[u8], &mut [u8]) -> Result<()> + Send + Sync,
{
    let (width, height) = dimensions;
    let destination_bytes = pixel_bytes(width, height, 4)?;
    let mut rgba = raw;
    if rgba.len() < destination_bytes {
        rgba.try_reserve_exact(destination_bytes - rgba.len())
            .context("the image is too large to hold in memory; use a smaller image")?;
        rgba.resize(destination_bytes, 0);
    }
    rewrite_rows_in_place(
        &mut rgba,
        height as usize,
        width as usize * layout.channels() * sample_bytes,
        width as usize * 4,
        initial,
        row_work,
    )?;
    rgba.truncate(destination_bytes);
    rgba.shrink_to_fit();
    cancellation.ensure_active()?;
    Ok(rgba)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputFormat;
    use crate::codec::render;
    use crate::codec::test_support::*;
    use image::codecs::png::PngDecoder;
    use image::{ExtendedColorType, ImageDecoder};
    use std::io::Cursor;

    #[test]
    fn grayscale_icc_conversion_keeps_alpha() {
        let profile = ColorProfile::new_gray_with_gamma(1.0).encode().unwrap();
        let source = png(
            &[128, 63],
            1,
            1,
            ExtendedColorType::La8,
            Some(profile),
            None,
        );
        let output = render(
            &source,
            &recipe(&source, 1, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        let pixel = rgba(&output);
        assert!((185..=190).contains(&pixel[0]));
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
        assert_eq!(pixel[3], 63);
        assert!(
            PngDecoder::new(Cursor::new(&output[..]))
                .unwrap()
                .icc_profile()
                .unwrap()
                .is_none()
        );
    }

    /// A JPEG that carries a colour profile is converted. The decoder's opaque-alpha shortcut
    /// applies only to sources that need no colour management.
    #[test]
    fn jpeg_colour_profile_is_applied() {
        let source = linear_icc_jpeg();
        let output = render(
            &source,
            &recipe(&source, 3, 3, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        let pixel = rgba(&output);
        assert!(
            (180..=195).contains(&pixel[0]),
            "the embedded profile is applied, not skipped: {pixel:?}"
        );
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
    }

    #[test]
    fn sixteen_bit_input_is_color_converted() {
        let profile = linear_profile();
        let pixels: Vec<u8> = [32_768u16, 32_768, 32_768, 16_384]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect();
        let source = png(
            &pixels,
            1,
            1,
            ExtendedColorType::Rgba16,
            Some(profile),
            None,
        );
        let output = render(
            &source,
            &recipe(&source, 1, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        let pixel = rgba(&output);
        assert!(
            pixel[..3]
                .iter()
                .all(|channel| (185..=190).contains(channel))
        );
        assert_eq!(pixel[3], 64);
    }

    #[test]
    fn invalid_icc_profiles_are_rejected() {
        let mut cmyk = ColorProfile::new_srgb().encode().unwrap();
        cmyk[16..20].copy_from_slice(b"CMYK");
        for profile in [cmyk, b"not an ICC profile".to_vec()] {
            let source = png(
                &[30, 80, 120],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(profile),
                None,
            );
            let recipe = recipe(&source, 1, 1, OutputFormat::Png, None);
            assert!(render(&source, &recipe, &active).is_err());
        }
    }

    #[test]
    fn png_gamma_is_applied_without_icc() {
        let mut source = png(&[128, 128, 128], 1, 1, ExtendedColorType::Rgb8, None, None);
        insert_png_chunk(&mut source, b"gAMA", &100_000u32.to_be_bytes());
        let output = render(
            &source,
            &recipe(&source, 1, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        assert!(
            rgba(&output)[..3]
                .iter()
                .all(|channel| (185..=190).contains(channel))
        );
    }
}
