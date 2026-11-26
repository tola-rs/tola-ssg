//! Normalizing decoded planes to the sRGB RGBA8 pixels every recipe starts from.

use std::borrow::Cow;

use image::ColorType;
use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};

use crate::cancellation::Cancellation;
use crate::rows::rewrite_rows_in_place;
use crate::{HOLD_IN_MEMORY, ImageError, ImageResult, pixel_bytes};

const PROFILE_UNSUPPORTED: &str =
    "the embedded color profile cannot be applied to this image; convert the image to sRGB";

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
    dims: (u32, u32),
    transform: Option<ColorTransform<'_>>,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let (w, _) = dims;
    cancel.ensure_active()?;
    if color == ColorType::Rgba8 && transform.is_none() {
        return Ok(raw);
    }
    let (layout, sample_bytes) = match color {
        ColorType::L8 => (Layout::Gray, 1),
        ColorType::La8 => (Layout::GrayAlpha, 1),
        ColorType::Rgb8 => (Layout::Rgb, 1),
        ColorType::Rgba8 => (Layout::Rgba, 1),
        ColorType::L16 => (Layout::Gray, 2),
        ColorType::La16 => (Layout::GrayAlpha, 2),
        ColorType::Rgb16 => (Layout::Rgb, 2),
        ColorType::Rgba16 => (Layout::Rgba, 2),
        _ => {
            return Err(ImageError::unreadable(
                "this image uses a color format Tola cannot resize; save it as 8-bit or 16-bit RGB",
            ));
        }
    };
    let gray = matches!(layout, Layout::Gray | Layout::GrayAlpha);
    let profile = match transform {
        Some(ColorTransform::Icc(icc)) => {
            let prof = ColorProfile::new_from_slice(icc.as_ref())
                .map_err(|err| ImageError::codec(super::PROFILE_DAMAGED, err))?;
            let expected_space = if gray {
                DataColorSpace::Gray
            } else {
                DataColorSpace::Rgb
            };
            if prof.color_space != expected_space {
                return Err(ImageError::unreadable(
                    "the embedded color profile does not match the image's color space; convert the image to sRGB",
                ));
            }
            Some(prof)
        }
        Some(ColorTransform::Gamma(gamma)) => {
            if gray {
                Some(ColorProfile::new_gray_with_gamma(gamma))
            } else {
                let mut prof = ColorProfile::new_srgb();
                prof.cicp = None;
                let curve = moxcms::curve_from_gamma(gamma);
                prof.red_trc = Some(curve.clone());
                prof.green_trc = Some(curve.clone());
                prof.blue_trc = Some(curve);
                Some(prof)
            }
        }
        None => None,
    };
    cancel.ensure_active()?;
    let Some(prof) = profile else {
        return rewrite_plane(
            raw,
            layout,
            sample_bytes,
            dims,
            cancel,
            || (),
            |(), input, output| {
                cancel.ensure_active()?;
                for (src, dst) in input
                    .chunks_exact(layout.channels() * sample_bytes)
                    .zip(output.as_chunks_mut::<4>().0.iter_mut())
                {
                    dst[0] = sample_at(src, 0, sample_bytes);
                    dst[1] = sample_at(src, if gray { 0 } else { 1 }, sample_bytes);
                    dst[2] = sample_at(src, if gray { 0 } else { 2 }, sample_bytes);
                    dst[3] = if layout.has_alpha() {
                        sample_at(src, layout.channels() - 1, sample_bytes)
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
    let color_layout = if gray { Layout::Gray } else { Layout::Rgb };
    let color_ch = color_layout.channels();
    let opts = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    if sample_bytes == 2 {
        let xform = prof
            .create_transform_16bit(color_layout, srgb, Layout::Rgba, opts)
            .map_err(|err| ImageError::codec(PROFILE_UNSUPPORTED, err))?;
        cancel.ensure_active()?;
        rewrite_plane(
            raw,
            layout,
            sample_bytes,
            dims,
            cancel,
            || {
                (
                    vec![0u16; w as usize * color_ch],
                    vec![0u16; w as usize * 4],
                )
            },
            |(in_row, out_row), input, output| {
                cancel.ensure_active()?;
                for (src, dst) in input
                    .chunks_exact(layout.channels() * 2)
                    .zip(in_row.chunks_exact_mut(color_ch))
                {
                    for (sample, b) in dst.iter_mut().zip(src.as_chunks::<2>().0.iter()) {
                        *sample = u16::from_ne_bytes([b[0], b[1]]);
                    }
                }
                xform
                    .transform(in_row, out_row)
                    .map_err(|err| ImageError::codec(PROFILE_UNSUPPORTED, err))?;
                for (dst, src) in output.iter_mut().zip(out_row.iter()) {
                    *dst = ((u32::from(*src) + 128) / 257) as u8;
                }
                if layout.has_alpha() {
                    restore_alpha(input, output, layout, 2);
                }
                Ok(())
            },
        )
    } else {
        let xform = prof
            .create_transform_8bit(color_layout, srgb, Layout::Rgba, opts)
            .map_err(|err| ImageError::codec(PROFILE_UNSUPPORTED, err))?;
        cancel.ensure_active()?;
        rewrite_plane(
            raw,
            layout,
            sample_bytes,
            dims,
            cancel,
            || {
                if layout.has_alpha() {
                    vec![0; w as usize * color_ch]
                } else {
                    Vec::new()
                }
            },
            |in_row, input, output| {
                cancel.ensure_active()?;
                let color_in = if layout.has_alpha() {
                    for (src, dst) in input
                        .chunks_exact(layout.channels())
                        .zip(in_row.chunks_exact_mut(color_ch))
                    {
                        dst.copy_from_slice(&src[..color_ch]);
                    }
                    in_row.as_slice()
                } else {
                    input
                };
                xform
                    .transform(color_in, output)
                    .map_err(|err| ImageError::codec(PROFILE_UNSUPPORTED, err))?;
                if layout.has_alpha() {
                    restore_alpha(input, output, layout, 1);
                }
                Ok(())
            },
        )
    }
}

/// One source sample as eight bits, narrowing a sixteen-bit sample around its midpoint.
fn sample_at(src: &[u8], ch: usize, sample_bytes: usize) -> u8 {
    let off = ch * sample_bytes;
    match sample_bytes {
        2 => ((u32::from(u16::from_ne_bytes([src[off], src[off + 1]])) + 128) / 257) as u8,
        _ => src[off],
    }
}

/// Copy each source pixel's alpha into the RGBA row, which the color transform leaves untouched.
fn restore_alpha(src_row: &[u8], dst_row: &mut [u8], layout: Layout, sample_bytes: usize) {
    let ch = layout.channels();
    for (src, dst) in src_row
        .chunks_exact(ch * sample_bytes)
        .zip(dst_row.as_chunks_mut::<4>().0.iter_mut())
    {
        dst[3] = sample_at(src, ch - 1, sample_bytes);
    }
}

/// Convert one decoded plane to RGBA8 in place so decode holds only one plane at a time.
fn rewrite_plane<S, I, F>(
    raw: Vec<u8>,
    layout: Layout,
    sample_bytes: usize,
    dims: (u32, u32),
    cancel: &dyn Cancellation,
    init: I,
    work: F,
) -> ImageResult<Vec<u8>>
where
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, &[u8], &mut [u8]) -> ImageResult<()> + Send + Sync,
{
    let (w, h) = dims;
    let dst_bytes = pixel_bytes(w, h, 4)?;
    let mut rgba = raw;
    if rgba.len() < dst_bytes {
        rgba.try_reserve_exact(dst_bytes - rgba.len())
            .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
        rgba.resize(dst_bytes, 0);
    }
    rewrite_rows_in_place(
        &mut rgba,
        h as usize,
        w as usize * layout.channels() * sample_bytes,
        w as usize * 4,
        init,
        work,
    )?;
    rgba.truncate(dst_bytes);
    rgba.shrink_to_fit();
    cancel.ensure_active()?;
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::OutputFormat;
    use crate::codec::render;
    use crate::codec::tests::test_support::*;
    use image::codecs::png::PngDecoder;
    use image::{ExtendedColorType, ImageDecoder};
    use std::io::Cursor;

    #[test]
    fn gray_icc_keeps_alpha() {
        let icc = ColorProfile::new_gray_with_gamma(1.0).encode().unwrap();
        let src = png(&[128, 63], 1, 1, ExtendedColorType::La8, Some(icc), None);
        let out = render_png(&src, 1, 1);
        let px = rgba(&out);
        assert!((185..=190).contains(&px[0]));
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
        assert_eq!(px[3], 63);
        assert!(
            PngDecoder::new(Cursor::new(&out[..]))
                .unwrap()
                .icc_profile()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn jpeg_icc_applied() {
        let src = linear_icc_jpeg();
        let out = render_png(&src, 3, 3);
        let px = rgba(&out);
        assert!(
            (180..=195).contains(&px[0]),
            "the embedded profile is applied, not skipped: {px:?}"
        );
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
    }

    #[test]
    fn icc_16bit_convert() {
        let icc = linear_profile();
        let px_bytes: Vec<u8> = [32_768u16, 32_768, 32_768, 16_384]
            .into_iter()
            .flat_map(u16::to_ne_bytes)
            .collect();
        let src = png(&px_bytes, 1, 1, ExtendedColorType::Rgba16, Some(icc), None);
        let out = render_png(&src, 1, 1);
        let px = rgba(&out);
        assert!(px[..3].iter().all(|ch| (185..=190).contains(ch)));
        assert_eq!(px[3], 64);
    }

    #[test]
    fn reject_invalid_icc() {
        let mut cmyk = ColorProfile::new_srgb().encode().unwrap();
        cmyk[16..20].copy_from_slice(b"CMYK");
        for icc in [cmyk, b"not an ICC profile".to_vec()] {
            let src = png(
                &[30, 80, 120],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(icc),
                None,
            );
            let r = recipe(&src, 1, 1, OutputFormat::Png, None);
            assert!(render(&src, &r, &NO_CANCELLATION).is_err());
        }
    }

    #[test]
    fn png_gamma_fallback() {
        let mut src = png(&[128, 128, 128], 1, 1, ExtendedColorType::Rgb8, None, None);
        insert_png_chunk(&mut src, b"gAMA", &100_000u32.to_be_bytes());
        let out = render_png(&src, 1, 1);
        assert!(rgba(&out)[..3].iter().all(|ch| (185..=190).contains(ch)));
    }
}
