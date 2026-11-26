//! Deterministic resampling: a separable integer convolution over fixed-point weights.
//!
//! Kernel values are evaluated with arithmetic alone. `f64::sin` and `f64::exp` resolve to the
//! platform's C library, whose results the derivative identity does not cover.

mod convolution;
mod nearest;
mod plane;
mod reduce;
mod weights;

use image::metadata::Orientation;

use crate::cancellation::Cancellation;
use crate::recipe::{Background, PixelRecipe, ResizeFilter};
use crate::rows::{any_row, for_each_row};
use crate::{ImageError, ImageMetadata, ImageResult, pixel_bytes, rgba_buffer};

use convolution::convolved_pixels;
use nearest::nearest_pixels;
use plane::{AlphaRepresentation, SourcePlane, source_plane};

impl PixelRecipe {
    /// Conservative pixel-plane peak for admission before decoding, in bytes.
    ///
    /// Includes the decoded source and assumes a materialized orientation crop. Alpha declarations
    /// select the wider convolution scratch even when decoding later finds every sample opaque.
    /// The calling thread's pool determines how many workers can hold convolution rows at once.
    /// Weight tables, codec scratch, encoded bytes, and retained caches are excluded.
    pub fn estimated_working_bytes(self, src: &ImageMetadata) -> u64 {
        let area = |w: u32, h: u32| u64::from(w) * u64::from(h);
        let out = area(self.width(), self.height()).saturating_mul(4);
        let Some(crop) = self.raster_crop() else {
            return out;
        };
        let decoded = area(src.width, src.height).saturating_mul(4);
        if (crop.width, crop.height) == (self.width(), self.height()) {
            return decoded.saturating_add(out);
        }
        let oriented = area(crop.width, crop.height).saturating_mul(4);
        if self.filter == ResizeFilter::Nearest {
            return decoded.saturating_add(oriented).saturating_add(out);
        }
        let (w, h, reduced) = match reduce::ReducedBlock::of(
            (crop.width, crop.height),
            (self.width(), self.height()),
        ) {
            Some(block) => {
                let (w, h) = block.dimensions(crop.width, crop.height);
                let bytes = area(w, h).saturating_mul(if src.has_alpha { 8 } else { 4 });
                (w, h, bytes)
            }
            None => (crop.width, crop.height, 0),
        };
        let inter = convolution::scratch_bytes(w, h, self, src.has_alpha);
        decoded
            .saturating_add(oriented)
            .saturating_add(reduced)
            .saturating_add(inter)
            .saturating_add(out)
    }
}

/// Output pixels of one recipe, and whether any of them stays transparent.
pub struct ResampledPixels {
    pixels: PixelRecipe,
    rgba: Vec<u8>,
    has_transparency: bool,
}

impl ResampledPixels {
    /// Whether any output pixel stays transparent.
    pub const fn has_transparency(&self) -> bool {
        self.has_transparency
    }

    /// The pixels themselves, as sRGB RGBA8 samples.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Bytes allocated for the RGBA pixel buffer, including spare capacity.
    pub fn allocated_bytes(&self) -> usize {
        self.rgba.capacity()
    }

    /// The geometry, filter, and background shared by encodings of these pixels.
    /// Its dimensions describe the row layout returned by [`Self::rgba`].
    pub const fn pixels(&self) -> PixelRecipe {
        self.pixels
    }

    /// Build pixels that were rendered directly, such as an SVG raster, rather than resampled.
    ///
    /// `rgba` is row-major straight-alpha sRGB, `width × height` pixels. A background
    /// composites it exactly as a resampled plane would.
    pub fn from_rendered(
        px: PixelRecipe,
        rgba: Vec<u8>,
        cancel: &dyn Cancellation,
    ) -> ImageResult<Self> {
        cancel.ensure_active()?;
        if rgba.len() != pixel_bytes(px.width(), px.height(), 4)? {
            return Err(ImageError::unreadable(
                "the rendered pixels do not match their recipe",
            ));
        }
        Self::assembled(px, rgba, false, cancel)
    }

    /// Finish one plane: composite its background, or record whether it stayed opaque.
    pub(crate) fn assembled(
        px: PixelRecipe,
        mut rgba: Vec<u8>,
        opaque: bool,
        cancel: &dyn Cancellation,
    ) -> ImageResult<Self> {
        if let Some(bg) = px.background {
            flatten_onto(&mut rgba, px.width(), bg, cancel)?;
            return Ok(Self {
                pixels: px,
                rgba,
                has_transparency: false,
            });
        }
        let has_transparency = !opaque
            && any_row(rgba.as_chunks::<4>().0, px.width() as usize, |p| {
                p[3] != u8::MAX
            });
        Ok(Self {
            pixels: px,
            rgba,
            has_transparency,
        })
    }
}

/// Crop, resample, and flatten decoded pixels into the pixels one recipe publishes.
pub(crate) fn resample(
    rgba: &[u8],
    w: u32,
    h: u32,
    orientation: Orientation,
    opaque: bool,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<ResampledPixels> {
    let Some(crop) = px.raster_crop() else {
        return Err(crate::ImageRequestError::VectorRecipeNotResampled.into());
    };
    let alpha = if opaque {
        AlphaRepresentation::Opaque
    } else {
        AlphaRepresentation::Straight
    };
    let mut plane = source_plane(rgba, w, h, orientation, crop, alpha, cancel)?;
    let identity = (px.width(), px.height()) == (plane.width(), plane.height());
    if !identity && px.filter != ResizeFilter::Nearest {
        plane.mark_opaque_if_covered(cancel)?;
    }
    let opaque = plane.alpha() == AlphaRepresentation::Opaque;
    let rgba = match plane {
        SourcePlane::Materialized { pixels: rgba, .. } if identity => {
            cancel.ensure_active()?;
            rgba
        }
        plane => resized_pixels(&plane, px, cancel)?,
    };
    ResampledPixels::assembled(px, rgba, opaque, cancel)
}

fn resized_pixels(
    plane: &SourcePlane<'_>,
    px: PixelRecipe,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let out_w = px.width();
    let out_h = px.height();
    if (out_w, out_h) == (plane.width(), plane.height()) {
        let mut out = rgba_buffer(out_w, out_h)?;
        for (y, row) in out.chunks_exact_mut(out_w as usize * 4).enumerate() {
            cancel.ensure_active()?;
            row.copy_from_slice(plane.row(y as u32));
        }
        return Ok(out);
    }
    if px.filter == ResizeFilter::Nearest {
        return nearest_pixels(plane, out_w, out_h, cancel);
    }
    convolved_pixels(plane, px, cancel)
}

/// Composite the pixels onto an opaque color, discarding transparency.
fn flatten_onto(
    rgba: &mut [u8],
    w: u32,
    bg: Background,
    cancel: &dyn Cancellation,
) -> ImageResult<()> {
    let backdrop = [i64::from(bg.red), i64::from(bg.green), i64::from(bg.blue)];
    for_each_row(
        rgba,
        w as usize * 4,
        || (),
        |(), _, row| {
            cancel.ensure_active()?;
            for p in row.as_chunks_mut::<4>().0.iter_mut() {
                let alpha = i64::from(p[3]);
                if alpha == u8::MAX as i64 {
                    continue;
                }
                for (ch, behind) in backdrop.into_iter().enumerate() {
                    let src = i64::from(p[ch]);
                    p[ch] = ((src * alpha + behind * (u8::MAX as i64 - alpha) + 127) / 255) as u8;
                }
                p[3] = u8::MAX;
            }
            Ok(())
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use self::test_support::plane;
    use super::convolution::convolved_reduced;
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::recipe::{ImageRecipe, OutputFormat, ResizeOperation, ResizeOptions};
    use crate::{ImageFormat, ImageMetadata};

    pub(crate) mod test_support {
        use crate::resample::plane::{AlphaRepresentation, SourcePlane};

        /// A window over pixels a test owns.
        pub(crate) fn plane(
            px: &[u8],
            w: u32,
            h: u32,
            alpha: AlphaRepresentation,
        ) -> SourcePlane<'_> {
            SourcePlane::Window {
                pixels: px,
                stride: w,
                origin: 0,
                width: w,
                height: h,
                alpha,
            }
        }
    }

    fn pixels(src_w: u32, src_h: u32, w: u32, h: u32, filter: ResizeFilter) -> PixelRecipe {
        resolve(src_w, src_h, w, h, filter, None).pixels()
    }

    fn resolve(
        src_w: u32,
        src_h: u32,
        w: u32,
        h: u32,
        filter: ResizeFilter,
        bg: Option<Background>,
    ) -> ImageRecipe {
        ImageRecipe::resolve(
            &ImageMetadata {
                width: src_w,
                height: src_h,
                format: ImageFormat::Png,
                has_alpha: true,
                is_lossy: false,
            },
            ResizeOptions {
                width: Some(w),
                height: Some(h),
                operation: ResizeOperation::Scale,
                format: OutputFormat::Png,
                filter,
                background: bg,
                ..ResizeOptions::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn low_alpha_keeps_color() {
        use image::{ExtendedColorType, ImageEncoder};

        for alpha in [1, 2, 8, 32, 128, 255] {
            let color = [100, 50, 200, alpha];
            for (w, h, out_w, out_h) in [(2, 2, 3, 3), (12, 12, 2, 2), (12, 12, 2, 3)] {
                let mut src = Vec::new();
                image::codecs::png::PngEncoder::new(&mut src)
                    .write_image(
                        &color.repeat((w * h) as usize),
                        w,
                        h,
                        ExtendedColorType::Rgba8,
                    )
                    .unwrap();
                for filter in [
                    ResizeFilter::Triangle,
                    ResizeFilter::CatmullRom,
                    ResizeFilter::Gaussian,
                    ResizeFilter::Lanczos3,
                ] {
                    let r = resolve(w, h, out_w, out_h, filter, None);
                    let enc = crate::render(&src, &r, &NO_CANCELLATION).unwrap();
                    let dec = image::load_from_memory(&enc).unwrap().to_rgba8();
                    for p in dec.pixels() {
                        assert_eq!(p.0, color, "{filter:?} {w}x{h} to {out_w}x{out_h}");
                    }
                }
            }
        }
    }

    #[test]
    fn coverage_weighted_average() {
        use image::{ExtendedColorType, ImageEncoder};

        struct Case {
            samples: &'static [[u8; 4]],
            w: u32,
            h: u32,
            expected: [u8; 4],
        }

        let cases = [
            Case {
                samples: &[[100, 50, 200, 255], [0, 0, 0, 255]],
                w: 2,
                h: 1,
                expected: [50, 25, 100, 255],
            },
            Case {
                samples: &[[200, 0, 0, 255], [0, 0, 0, 255]],
                w: 1,
                h: 2,
                expected: [100, 0, 0, 255],
            },
            Case {
                samples: &[[100, 50, 200, 2], [0, 0, 0, 0]],
                w: 2,
                h: 1,
                expected: [100, 50, 200, 1],
            },
            Case {
                samples: &[
                    [100, 50, 200, 255],
                    [9, 80, 211, 0],
                    [100, 50, 200, 255],
                    [100, 50, 200, 255],
                ],
                w: 2,
                h: 2,
                expected: [100, 50, 200, 191],
            },
        ];
        for case in cases {
            let mut src = Vec::new();
            image::codecs::png::PngEncoder::new(&mut src)
                .write_image(
                    case.samples.as_flattened(),
                    case.w,
                    case.h,
                    ExtendedColorType::Rgba8,
                )
                .unwrap();
            for filter in [
                ResizeFilter::Triangle,
                ResizeFilter::CatmullRom,
                ResizeFilter::Gaussian,
                ResizeFilter::Lanczos3,
            ] {
                let r = resolve(case.w, case.h, 1, 1, filter, None);
                let enc = crate::render(&src, &r, &NO_CANCELLATION).unwrap();
                let pub_px = image::load_from_memory(&enc).unwrap().to_rgba8();
                assert_eq!(
                    pub_px.as_raw().as_slice(),
                    case.expected.as_slice(),
                    "{filter:?} over {:?}",
                    case.samples
                );
            }
        }
    }

    #[test]
    fn covered_box_channel_sums() {
        use image::{ExtendedColorType, ImageEncoder};

        let color = [255u8, 192, 128, 254];
        let mut src = Vec::new();
        image::codecs::png::PngEncoder::new(&mut src)
            .write_image(
                &color.repeat(1000 * 1000),
                1000,
                1000,
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        let r = resolve(1000, 1000, 1, 1, ResizeFilter::Triangle, None);
        let enc = crate::render(&src, &r, &NO_CANCELLATION).unwrap();
        assert_eq!(
            image::load_from_memory(&enc).unwrap().to_rgba8().as_raw(),
            &color
        );
    }

    #[test]
    fn identity_returns_src() {
        let src = [1u8, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let plane = plane(&src, 2, 2, AlphaRepresentation::Opaque);
        let res = resized_pixels(
            &plane,
            pixels(2, 2, 2, 2, ResizeFilter::Lanczos3),
            &NO_CANCELLATION,
        )
        .unwrap();
        assert_eq!(res, src);
    }

    #[test]
    fn box_shift_within_one_px() {
        let (w, h) = (96u32, 64u32);
        let mut src = Vec::new();
        for y in 0..h {
            for x in 0..w {
                src.extend_from_slice(&[
                    (x * 255 / (w - 1)) as u8,
                    (y * 255 / (h - 1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        let src = plane(&src, w, h, AlphaRepresentation::Opaque);
        for (out_w, out_h) in [(12u32, 8u32), (6, 4), (24, 16)] {
            let r = pixels(w, h, out_w, out_h, ResizeFilter::Lanczos3);
            let fair = convolved_reduced(&src, r, &NO_CANCELLATION).unwrap();
            let staged = convolved_pixels(&src, r, &NO_CANCELLATION).unwrap();
            let step = 255.0 / (w - 1) as f64;
            let bound = ((step * (w as f64 / out_w as f64)) as u8).saturating_add(1);
            let worst = fair
                .iter()
                .zip(&staged)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or_default();
            assert!(
                worst <= bound,
                "{out_w}x{out_h} moved {worst} steps, past the {bound}-step bound"
            );
        }
    }

    #[test]
    fn box_keeps_translucent_color() {
        let color = [200u8, 100, 50, 128];
        let src = color.repeat(12 * 12);
        let src = plane(&src, 12, 12, AlphaRepresentation::Straight);
        // These geometries run the horizontal and vertical kernel axes first, respectively.
        for (w, h) in [(2, 2), (2, 3)] {
            let res = resized_pixels(
                &src,
                pixels(12, 12, w, h, ResizeFilter::Lanczos3),
                &NO_CANCELLATION,
            )
            .unwrap();
            for p in res.as_chunks::<4>().0 {
                assert_eq!(p[3], color[3]);
                for ch in 0..3 {
                    assert!(
                        p[ch].abs_diff(color[ch]) <= 1,
                        "{w}x{h}: {p:?} changed translucent color"
                    );
                }
            }
        }
    }

    /// Negative-lobe overshoot must survive pass 1; clamping early shifts hue on pass 2.
    #[test]
    fn alpha_overshoot_keeps_color() {
        let color = [59u8, 137, 223, 255];
        let transparent = [59, 137, 223, 0];
        let src_px = [transparent, color, color, color];
        let src = plane(src_px.as_flattened(), 2, 2, AlphaRepresentation::Straight);
        for filter in [ResizeFilter::CatmullRom, ResizeFilter::Lanczos3] {
            let res = resized_pixels(&src, pixels(2, 2, 5, 5, filter), &NO_CANCELLATION).unwrap();
            for p in res.as_chunks::<4>().0 {
                if p[3] < 255 {
                    continue;
                }
                for ch in 0..3 {
                    assert!(
                        p[ch].abs_diff(color[ch]) <= 2,
                        "{filter:?}: {p:?} shifted a constant hue"
                    );
                }
            }
        }
    }

    #[test]
    fn flatten_composites_bg() {
        let mut px = [200u8, 100, 50, 0, 10, 20, 30, 128, 255, 255, 255, 255];
        flatten_onto(
            &mut px,
            3,
            Background {
                red: 255,
                green: 255,
                blue: 255,
            },
            &NO_CANCELLATION,
        )
        .unwrap();
        assert_eq!(
            px,
            [255, 255, 255, 255, 132, 137, 142, 255, 255, 255, 255, 255]
        );
    }

    /// Full coverage must be exact: opaque straight-alpha input matches `Opaque` on both pass orders.
    #[test]
    fn full_coverage_matches_opaque() {
        let w = 24u32;
        let h = 3;
        let mut src = Vec::new();
        for i in 0..(w * h) as usize {
            src.extend_from_slice(&[
                (i * 7 % 251) as u8,
                (i * 13 % 241) as u8,
                (i * 29 % 239) as u8,
                255,
            ]);
        }
        for filter in [
            ResizeFilter::Triangle,
            ResizeFilter::CatmullRom,
            ResizeFilter::Gaussian,
            ResizeFilter::Lanczos3,
        ] {
            for (out_w, out_h) in [(6u32, 2u32), (12, 1), (96, 3), (2, 1)] {
                let covered = resample(
                    &src,
                    w,
                    h,
                    Orientation::NoTransforms,
                    false,
                    pixels(w, h, out_w, out_h, filter),
                    &NO_CANCELLATION,
                )
                .unwrap();
                let opaque = resample(
                    &src,
                    w,
                    h,
                    Orientation::NoTransforms,
                    true,
                    pixels(w, h, out_w, out_h, filter),
                    &NO_CANCELLATION,
                )
                .unwrap();
                assert_eq!(
                    covered.rgba(),
                    opaque.rgba(),
                    "{filter:?} at {out_w}x{out_h}"
                );
            }
        }
    }

    #[test]
    fn resample_cancel() {
        let src = [128u8, 128, 128, 255].repeat(600 * 600);
        let req = std::sync::atomic::AtomicUsize::new(0);
        let cancel = || req.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 64;
        let outcome = resized_pixels(
            &plane(&src, 600, 600, AlphaRepresentation::Opaque),
            pixels(600, 600, 150, 150, ResizeFilter::Lanczos3),
            &cancel,
        );
        assert!(matches!(&outcome, Err(ImageError::Cancelled(_))));
    }

    #[test]
    fn transparent_no_bleed() {
        for (w, h, out_w, out_h, filter) in [
            (2, 1, 1, 1, ResizeFilter::Triangle),
            (12, 12, 2, 2, ResizeFilter::Lanczos3),
            (12, 12, 2, 3, ResizeFilter::Lanczos3),
        ] {
            let src = [255u8, 0, 0, 128, 0, 255, 0, 0].repeat((w * h / 2) as usize);
            let src = plane(&src, w, h, AlphaRepresentation::Straight);
            let res =
                resized_pixels(&src, pixels(w, h, out_w, out_h, filter), &NO_CANCELLATION).unwrap();
            for p in res.as_chunks::<4>().0 {
                assert_eq!(*p, [255, 0, 0, 64]);
            }
        }
    }

    #[test]
    fn wider_kernels_spread() {
        let mut src = vec![0u8; 8 * 4];
        for p in src.as_chunks_mut::<4>().0.iter_mut() {
            p[3] = 255;
        }
        src[4 * 4..4 * 4 + 3].copy_from_slice(&[255, 255, 255]);
        let plane = plane(&src, 8, 1, AlphaRepresentation::Opaque);
        let spread = |filter| {
            let res =
                resized_pixels(&plane, pixels(8, 1, 32, 1, filter), &NO_CANCELLATION).unwrap();
            res.as_chunks::<4>().0.iter().filter(|p| p[0] > 0).count()
        };
        let nearest = spread(ResizeFilter::Nearest);
        let tri = spread(ResizeFilter::Triangle);
        let lanczos = spread(ResizeFilter::Lanczos3);
        assert_eq!(nearest, 4);
        assert!(tri > nearest, "triangle covers {tri} px");
        assert!(lanczos > tri, "Lanczos covers {lanczos} px");
    }
}
