//! Deterministic resampling: a separable integer convolution over fixed-point weights.
//!
//! Kernel values are evaluated with arithmetic alone. `f64::sin` and `f64::exp` resolve to the
//! platform's C library, whose results the derivative identity does not cover.

mod convolution;
mod nearest;
mod plane;
mod reduce;
mod weights;

use anyhow::Result;
use image::metadata::Orientation;

use crate::cancellation::Cancellation;
use crate::recipe::{Background, PixelRecipe, ResizeFilter};
use crate::rgba_buffer;
use crate::rows::{any_row, for_each_row};

use convolution::convolved_pixels;
use nearest::nearest_pixels;
use plane::{AlphaRepresentation, SourcePlane, source_plane};

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

    /// The geometry, filter, and background shared by encodings of these pixels.
    /// Its dimensions describe the row layout returned by [`Self::rgba`].
    pub const fn pixels(&self) -> PixelRecipe {
        self.pixels
    }
}

/// Crop, resample, and flatten decoded pixels into the pixels one recipe publishes.
pub(crate) fn resample(
    rgba: &[u8],
    width: u32,
    height: u32,
    orientation: Orientation,
    opaque: bool,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<ResampledPixels> {
    let alpha = if opaque {
        AlphaRepresentation::Opaque
    } else {
        AlphaRepresentation::Straight
    };
    let mut plane = source_plane(
        rgba,
        width,
        height,
        orientation,
        pixels.crop,
        alpha,
        cancellation,
    )?;
    let identity = (pixels.width(), pixels.height()) == (plane.width(), plane.height());
    if !identity && pixels.filter != ResizeFilter::Nearest {
        plane.mark_opaque_if_covered(cancellation)?;
    }
    let opaque = plane.alpha() == AlphaRepresentation::Opaque;
    let mut rgba = match plane {
        SourcePlane::Materialized { pixels: rgba, .. } if identity => {
            cancellation.ensure_active()?;
            rgba
        }
        plane => resized_pixels(&plane, pixels, cancellation)?,
    };
    if let Some(background) = pixels.background {
        flatten_onto(&mut rgba, pixels.width(), background, cancellation)?;
        return Ok(ResampledPixels {
            pixels,
            rgba,
            has_transparency: false,
        });
    }
    let has_transparency = !opaque
        && any_row(rgba.as_chunks::<4>().0, pixels.width() as usize, |pixel| {
            pixel[3] != u8::MAX
        });
    Ok(ResampledPixels {
        pixels,
        rgba,
        has_transparency,
    })
}

fn resized_pixels(
    plane: &SourcePlane<'_>,
    pixels: PixelRecipe,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let output_width = pixels.width();
    let output_height = pixels.height();
    if (output_width, output_height) == (plane.width(), plane.height()) {
        let mut output = rgba_buffer(output_width, output_height)?;
        for (y, row) in output
            .chunks_exact_mut(output_width as usize * 4)
            .enumerate()
        {
            cancellation.ensure_active()?;
            row.copy_from_slice(plane.row(y as u32));
        }
        return Ok(output);
    }
    if pixels.filter == ResizeFilter::Nearest {
        return nearest_pixels(plane, output_width, output_height, cancellation);
    }
    convolved_pixels(plane, pixels, cancellation)
}

/// Composite the pixels onto an opaque color, discarding transparency.
fn flatten_onto(
    rgba: &mut [u8],
    width: u32,
    background: Background,
    cancellation: &dyn Cancellation,
) -> Result<()> {
    let backdrop = [
        i64::from(background.red),
        i64::from(background.green),
        i64::from(background.blue),
    ];
    for_each_row(
        rgba,
        width as usize * 4,
        || (),
        |(), _, row| {
            cancellation.ensure_active()?;
            for pixel in row.as_chunks_mut::<4>().0.iter_mut() {
                let alpha = i64::from(pixel[3]);
                if alpha == u8::MAX as i64 {
                    continue;
                }
                for (channel, behind) in backdrop.into_iter().enumerate() {
                    let source = i64::from(pixel[channel]);
                    pixel[channel] =
                        ((source * alpha + behind * (u8::MAX as i64 - alpha) + 127) / 255) as u8;
                }
                pixel[3] = u8::MAX;
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::convolution::convolved_reduced;
    use super::*;
    use crate::ImageCancelled;
    use crate::recipe::{ImageRecipe, OutputFormat, ResizeOperation, ResizeOptions};
    use crate::resample::test_support::{active, plane};
    use crate::{ImageFormat, ImageMetadata};

    fn pixels(
        source_width: u32,
        source_height: u32,
        width: u32,
        height: u32,
        filter: ResizeFilter,
    ) -> PixelRecipe {
        resolve(source_width, source_height, width, height, filter, None).pixels()
    }

    fn resolve(
        source_width: u32,
        source_height: u32,
        width: u32,
        height: u32,
        filter: ResizeFilter,
        background: Option<Background>,
    ) -> ImageRecipe {
        ImageRecipe::resolve(
            &ImageMetadata {
                width: source_width,
                height: source_height,
                format: ImageFormat::Png,
                has_alpha: true,
                is_lossy: false,
            },
            ResizeOptions {
                width: Some(width),
                height: Some(height),
                operation: ResizeOperation::Scale,
                format: OutputFormat::Png,
                quality: None,
                filter,
                background,
            },
        )
        .unwrap()
    }

    #[test]
    fn low_coverage_preserves_constant_color() {
        use image::{ExtendedColorType, ImageEncoder};

        for alpha in [1, 2, 8, 32, 128, 255] {
            let color = [100, 50, 200, alpha];
            for (width, height, output_width, output_height) in
                [(2, 2, 3, 3), (12, 12, 2, 2), (12, 12, 2, 3)]
            {
                let mut source = Vec::new();
                image::codecs::png::PngEncoder::new(&mut source)
                    .write_image(
                        &color.repeat((width * height) as usize),
                        width,
                        height,
                        ExtendedColorType::Rgba8,
                    )
                    .unwrap();
                for filter in [
                    ResizeFilter::Triangle,
                    ResizeFilter::CatmullRom,
                    ResizeFilter::Gaussian,
                    ResizeFilter::Lanczos3,
                ] {
                    let recipe = resolve(width, height, output_width, output_height, filter, None);
                    let encoded = crate::render(&source, &recipe, &active).unwrap();
                    let decoded = image::load_from_memory(&encoded).unwrap().to_rgba8();
                    for pixel in decoded.pixels() {
                        assert_eq!(
                            pixel.0, color,
                            "{filter:?} {width}x{height} to {output_width}x{output_height}"
                        );
                    }
                }
            }
        }
    }

    /// One covered output pixel is the coverage-weighted mean of the samples beneath it: a
    /// transparent sample contributes neither colour nor coverage, and a kernel only decides
    /// which samples are covered. Every value below is that mean computed by hand.
    #[test]
    fn coverage_weighted_average_decides_pixels() {
        use image::{ExtendedColorType, ImageEncoder};

        /// One source image and the pixel a covered output averages to.
        struct Case {
            samples: &'static [[u8; 4]],
            width: u32,
            height: u32,
            expected: [u8; 4],
        }

        let cases = [
            // The mean of an opaque pair, both samples at equal distance from the axis center.
            Case {
                samples: &[[100, 50, 200, 255], [0, 0, 0, 255]],
                width: 2,
                height: 1,
                expected: [50, 25, 100, 255],
            },
            // The same mean where the shared samples lie on the vertical axis.
            Case {
                samples: &[[200, 0, 0, 255], [0, 0, 0, 255]],
                width: 1,
                height: 2,
                expected: [100, 0, 0, 255],
            },
            // A translucent sample keeps its own colour at half its coverage.
            Case {
                samples: &[[100, 50, 200, 2], [0, 0, 0, 0]],
                width: 2,
                height: 1,
                expected: [100, 50, 200, 1],
            },
            // One transparent sample of four: the mean of the three opaque ones.
            Case {
                samples: &[
                    [100, 50, 200, 255],
                    [9, 80, 211, 0],
                    [100, 50, 200, 255],
                    [100, 50, 200, 255],
                ],
                width: 2,
                height: 2,
                expected: [100, 50, 200, 191],
            },
        ];
        for case in cases {
            let mut source = Vec::new();
            image::codecs::png::PngEncoder::new(&mut source)
                .write_image(
                    case.samples.as_flattened(),
                    case.width,
                    case.height,
                    ExtendedColorType::Rgba8,
                )
                .unwrap();
            for filter in [
                ResizeFilter::Triangle,
                ResizeFilter::CatmullRom,
                ResizeFilter::Gaussian,
                ResizeFilter::Lanczos3,
            ] {
                let recipe = resolve(case.width, case.height, 1, 1, filter, None);
                let encoded = crate::render(&source, &recipe, &active).unwrap();
                let published = image::load_from_memory(&encoded).unwrap().to_rgba8();
                assert_eq!(
                    published.as_raw().as_slice(),
                    case.expected.as_slice(),
                    "{filter:?} over {:?}",
                    case.samples
                );
            }
        }
    }

    #[test]
    fn covered_box_preserves_channel_sums() {
        use image::{ExtendedColorType, ImageEncoder};

        let color = [255u8, 192, 128, 254];
        let mut source = Vec::new();
        image::codecs::png::PngEncoder::new(&mut source)
            .write_image(
                &color.repeat(1000 * 1000),
                1000,
                1000,
                ExtendedColorType::Rgba8,
            )
            .unwrap();
        let recipe = resolve(1000, 1000, 1, 1, ResizeFilter::Triangle, None);
        let encoded = crate::render(&source, &recipe, &active).unwrap();
        assert_eq!(
            image::load_from_memory(&encoded)
                .unwrap()
                .to_rgba8()
                .as_raw(),
            &color
        );
    }

    #[test]
    fn identity_geometry_returns_source_pixels() {
        let source = [1u8, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let plane = plane(&source, 2, 2, AlphaRepresentation::Opaque);
        let resized =
            resized_pixels(&plane, pixels(2, 2, 2, 2, ResizeFilter::Lanczos3), &active).unwrap();
        assert_eq!(resized, source);
    }

    /// The box stage may shift a smooth source by less than the source pixel it averages over —
    /// the phase the prefilter trades for antialiasing — and must not move it further, in either
    /// direction, at any output geometry.
    #[test]
    fn box_stage_shifts_within_one_pixel() {
        let (width, height) = (96u32, 64u32);
        let mut source = Vec::new();
        for y in 0..height {
            for x in 0..width {
                source.extend_from_slice(&[
                    (x * 255 / (width - 1)) as u8,
                    (y * 255 / (height - 1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        let source = plane(&source, width, height, AlphaRepresentation::Opaque);
        for (out_width, out_height) in [(12u32, 8u32), (6, 4), (24, 16)] {
            let recipe = pixels(width, height, out_width, out_height, ResizeFilter::Lanczos3);
            let fair = convolved_reduced(&source, recipe, &active).unwrap();
            let staged = convolved_pixels(&source, recipe, &active).unwrap();
            // One source column of a 0..255 ramp, spread over the source pixels that land in one
            // output pixel: the most a half-source-pixel shift can move an output sample.
            let step = 255.0 / (width - 1) as f64;
            let bound = ((step * (width as f64 / out_width as f64)) as u8).saturating_add(1);
            let worst = fair
                .iter()
                .zip(&staged)
                .map(|(left, right)| left.abs_diff(*right))
                .max()
                .unwrap_or_default();
            assert!(
                worst <= bound,
                "{out_width}x{out_height} moved {worst} steps, past the {bound}-step bound"
            );
        }
    }

    #[test]
    fn box_kernel_preserves_translucent_color() {
        let color = [200u8, 100, 50, 128];
        let source = color.repeat(12 * 12);
        let source = plane(&source, 12, 12, AlphaRepresentation::Straight);
        // These geometries run the horizontal and vertical kernel axes first, respectively.
        for (width, height) in [(2, 2), (2, 3)] {
            let resized = resized_pixels(
                &source,
                pixels(12, 12, width, height, ResizeFilter::Lanczos3),
                &active,
            )
            .unwrap();
            for pixel in resized.as_chunks::<4>().0 {
                assert_eq!(pixel[3], color[3]);
                for channel in 0..3 {
                    assert!(
                        pixel[channel].abs_diff(color[channel]) <= 1,
                        "{width}x{height}: {pixel:?} changed the translucent color"
                    );
                }
            }
        }
    }

    /// A negative-lobe kernel overshoots coverage past opaque at a transparent edge. That overshoot
    /// must survive the first pass: clipping it there leaves the second pass restoring color from a
    /// coverage the first never produced, which visibly shifts a constant hue toward the kernel's
    /// halo. Both axes filter here, so the first-pass lane is where the clip would land.
    #[test]
    fn alpha_overshoot_preserves_color() {
        let color = [59u8, 137, 223, 255];
        let transparent = [59, 137, 223, 0];
        let source_pixels = [transparent, color, color, color];
        let source = plane(
            source_pixels.as_flattened(),
            2,
            2,
            AlphaRepresentation::Straight,
        );
        for filter in [ResizeFilter::CatmullRom, ResizeFilter::Lanczos3] {
            let resized = resized_pixels(&source, pixels(2, 2, 5, 5, filter), &active).unwrap();
            for pixel in resized.as_chunks::<4>().0 {
                // Partial coverage carries its own premultiplied rounding, so the visible-color
                // invariant is only meaningful where the output is fully covered.
                if pixel[3] < 255 {
                    continue;
                }
                for channel in 0..3 {
                    assert!(
                        pixel[channel].abs_diff(color[channel]) <= 2,
                        "{filter:?}: {pixel:?} shifted a constant hue"
                    );
                }
            }
        }
    }

    #[test]
    fn flattening_composites_onto_background() {
        let mut pixels = [200u8, 100, 50, 0, 10, 20, 30, 128, 255, 255, 255, 255];
        flatten_onto(
            &mut pixels,
            3,
            Background {
                red: 255,
                green: 255,
                blue: 255,
            },
            &active,
        )
        .unwrap();
        assert_eq!(
            pixels,
            [255, 255, 255, 255, 132, 137, 142, 255, 255, 255, 255, 255]
        );
    }

    /// Full coverage must be exact: a transparent source whose alpha is opaque everywhere filters
    /// to the same samples as a source without an alpha channel. Both pass orders are covered: the
    /// second output geometry runs the vertical axis first.
    #[test]
    fn full_coverage_filters_like_opaque() {
        let width = 24u32;
        let height = 3;
        let mut source = Vec::new();
        for index in 0..(width * height) as usize {
            source.extend_from_slice(&[
                (index * 7 % 251) as u8,
                (index * 13 % 241) as u8,
                (index * 29 % 239) as u8,
                255,
            ]);
        }
        // The filters whose weights consult opacity; nearest and an unchanged geometry reach their
        // pixels without it, so comparing them would only repeat one code path twice.
        for filter in [
            ResizeFilter::Triangle,
            ResizeFilter::CatmullRom,
            ResizeFilter::Gaussian,
            ResizeFilter::Lanczos3,
        ] {
            for (out_width, out_height) in [(6u32, 2u32), (12, 1), (96, 3), (2, 1)] {
                let covered = resample(
                    &source,
                    width,
                    height,
                    Orientation::NoTransforms,
                    false,
                    pixels(width, height, out_width, out_height, filter),
                    &active,
                )
                .unwrap();
                let opaque = resample(
                    &source,
                    width,
                    height,
                    Orientation::NoTransforms,
                    true,
                    pixels(width, height, out_width, out_height, filter),
                    &active,
                )
                .unwrap();
                assert_eq!(
                    covered.rgba(),
                    opaque.rgba(),
                    "weighting a sample by full coverage changes nothing with {filter:?} at {out_width}x{out_height}"
                );
            }
        }
    }

    #[test]
    fn resampling_observes_cancellation() {
        let source = [128u8, 128, 128, 255].repeat(600 * 600);
        let requested = std::sync::atomic::AtomicUsize::new(0);
        // Cancelled once the resize is under way, so work started after the request sees it.
        let cancellation = || requested.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 64;
        let outcome = resized_pixels(
            &plane(&source, 600, 600, AlphaRepresentation::Opaque),
            pixels(600, 600, 150, 150, ResizeFilter::Lanczos3),
            &cancellation,
        );
        assert!(
            matches!(&outcome, Err(error) if error.downcast_ref::<ImageCancelled>().is_some()),
            "an interrupted resize reports the cancellation",
        );
    }

    #[test]
    fn transparent_neighbors_do_not_bleed() {
        for (width, height, output_width, output_height, filter) in [
            (2, 1, 1, 1, ResizeFilter::Triangle),
            (12, 12, 2, 2, ResizeFilter::Lanczos3),
            (12, 12, 2, 3, ResizeFilter::Lanczos3),
        ] {
            let source = [255u8, 0, 0, 128, 0, 255, 0, 0].repeat((width * height / 2) as usize);
            let source = plane(&source, width, height, AlphaRepresentation::Straight);
            let resized = resized_pixels(
                &source,
                pixels(width, height, output_width, output_height, filter),
                &active,
            )
            .unwrap();
            for pixel in resized.as_chunks::<4>().0 {
                assert_eq!(*pixel, [255, 0, 0, 64]);
            }
        }
    }

    #[test]
    fn wider_kernels_spread_one_sample_further() {
        let mut source = vec![0u8; 8 * 4];
        for pixel in source.as_chunks_mut::<4>().0.iter_mut() {
            pixel[3] = 255;
        }
        source[4 * 4..4 * 4 + 3].copy_from_slice(&[255, 255, 255]);
        let plane = plane(&source, 8, 1, AlphaRepresentation::Opaque);
        let spread = |filter| {
            let resized = resized_pixels(&plane, pixels(8, 1, 32, 1, filter), &active).unwrap();
            resized
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[0] > 0)
                .count()
        };
        let nearest = spread(ResizeFilter::Nearest);
        let triangle = spread(ResizeFilter::Triangle);
        let lanczos = spread(ResizeFilter::Lanczos3);
        assert_eq!(nearest, 4);
        assert!(
            triangle > nearest,
            "the triangle kernel covers {triangle} pixels"
        );
        assert!(
            lanczos > triangle,
            "the Lanczos kernel covers {lanczos} pixels"
        );
    }
}

/// Cancellation and source windows the `resample` module's tests share.
#[cfg(test)]
pub(crate) mod test_support {
    use super::plane::{AlphaRepresentation, SourcePlane};

    /// A cancellation request that never fires, so a test exercises the pixels alone.
    pub(super) fn active() -> bool {
        false
    }

    /// A window over pixels a test owns.
    pub(super) fn plane(
        pixels: &[u8],
        width: u32,
        height: u32,
        alpha: AlphaRepresentation,
    ) -> SourcePlane<'_> {
        SourcePlane::Window {
            pixels,
            stride: width,
            origin: 0,
            width,
            height,
            alpha,
        }
    }
}
