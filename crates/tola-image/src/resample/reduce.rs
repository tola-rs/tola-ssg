//! The box stage: whole-block averaging that runs before the kernel on a large shrink.

use anyhow::{Context, Result};
use fearless_simd::prelude::*;
use fearless_simd::{Level, dispatch, u8x16, u16x8};

use crate::cancellation::Cancellation;
use crate::rows::for_each_row;
use crate::{pixel_bytes, rgba_buffer};

use super::plane::{AlphaRepresentation, CoveredPlane, SourcePlane, premultiply};

/// How much larger than its target a source axis must be before the box stage runs.
///
/// libvips' `reduce` will not run a kernel alone past a shrink of three and offers a box `shrink`
/// in front of it; Pillow pre-reduces by `int(source / target / gap)` and calls three the point
/// where the result stops differing from fair resampling.
const REDUCE_GAP: f64 = 3.0;

/// Shared block factors bound opaque `255 * count` sums in `u32`.
/// Covered products use `u64` sums without changing the box geometry.
const MAX_BLOCK_PIXELS: u32 = (u32::MAX / 255) & !0xffff;

/// The whole factor one source axis is averaged by before the kernel runs.
fn reduced_factor(input: u32, output: u32, gap: f64) -> u32 {
    let factor = (f64::from(input) / f64::from(output) / gap).floor();
    if !factor.is_finite() || factor < 2.0 {
        return 1;
    }
    (factor as u32).min(input)
}

/// The whole factors one plane is averaged by before the kernel runs.
#[derive(Clone, Copy)]

pub(super) struct ReducedBlock {
    horizontal: u32,
    vertical: u32,
}

impl ReducedBlock {
    /// The block one resize averages by, or `None` when neither axis is worth a box stage.
    pub(super) fn of(input: (u32, u32), output: (u32, u32)) -> Option<Self> {
        let mut block = Self {
            horizontal: reduced_factor(input.0, output.0, REDUCE_GAP),
            vertical: reduced_factor(input.1, output.1, REDUCE_GAP),
        };
        if block.horizontal == 1 && block.vertical == 1 {
            return None;
        }
        // Halve the wider factor until the opaque `u32` sum fits.
        while u64::from(block.horizontal) * u64::from(block.vertical) > u64::from(MAX_BLOCK_PIXELS)
        {
            if block.horizontal >= block.vertical {
                block.horizontal = block.horizontal.div_ceil(2);
            } else {
                block.vertical = block.vertical.div_ceil(2);
            }
        }
        Some(block)
    }
}

/// Sum four channels over one opaque block, on the widest vectors the running CPU offers.
///
/// Four pixels are one 16-byte load, so a block of at most 255 pixels accumulates in `u16` without
/// an overflow; a block shorter than four pixels is finished by the scalar remainder.
fn opaque_block_sums(level: Level, block: &[u8]) -> [u32; 4] {
    let whole = block.len() / 16 * 16;
    let (vector, tail) = block.split_at(whole);
    let mut sums = [0u32; 4];
    dispatch!(level, simd => {
        let mut lower = u16x8::splat(simd, 0);
        let mut upper = u16x8::splat(simd, 0);
        for chunk in vector.as_chunks::<16>().0 {
            let (below, above) = u8x16::from_slice(simd, chunk).widen();
            lower += below;
            upper += above;
        }
        // A 16-byte load holds four pixels: even ones reach the lower lanes and odd ones the upper
        // lanes, so one channel's total is its two lanes added together.
        let lanes = (lower + upper).to_array();
        for (total, (even, odd)) in sums.iter_mut().zip(lanes.iter().zip(&lanes[4..])) {
            *total = u32::from(*even) + u32::from(*odd);
        }
    });
    for pixel in tail.as_chunks::<4>().0 {
        for (total, sample) in sums.iter_mut().zip(pixel) {
            *total += u32::from(*sample);
        }
    }
    sums
}

/// How many source columns one reduced column covers.
fn block_columns(x: u32, width: u32, horizontal: u32) -> u32 {
    let first = x * horizontal;
    (first + horizontal).min(width) - first
}

/// Edge blocks average only the source pixels that exist.
pub(super) fn box_averaged(
    plane: &SourcePlane<'_>,
    block: ReducedBlock,
    cancellation: &dyn Cancellation,
) -> Result<SourcePlane<'static>> {
    let (horizontal, vertical) = (block.horizontal, block.vertical);
    let width = plane.width().div_ceil(horizontal);
    let height = plane.height().div_ceil(vertical);
    let mut pixels = rgba_buffer(width, height)?;
    let level = Level::new();
    // One divisor per output column, so the row loop divides once per output pixel and never
    // per source pixel.
    let columns: Vec<u32> = (0..width)
        .map(|x| block_columns(x, plane.width(), horizontal))
        .collect();
    let row_bytes = width as usize * 4;
    for_each_row(
        &mut pixels,
        row_bytes,
        || vec![0u32; width as usize * 4],
        |totals, y, row| {
            cancellation.ensure_active()?;
            totals.fill(0);
            let first = y as u32 * vertical;
            let last = (first + vertical).min(plane.height());
            for source in first..last {
                cancellation.ensure_active()?;
                let source = plane.row(source);
                let mut target = 0;
                for (index, block) in source
                    .as_chunks::<4>()
                    .0
                    .chunks(horizontal as usize)
                    .enumerate()
                {
                    // More than 255 pixels would overflow the vector's `u16` sums.
                    let sum = if (4..=255).contains(&horizontal) {
                        let bytes = index * horizontal as usize * 4;
                        opaque_block_sums(level, &source[bytes..][..block.len() * 4])
                    } else {
                        let mut sum = [0u32; 4];
                        for pixel in block {
                            for (total, sample) in sum.iter_mut().zip(pixel) {
                                *total += u32::from(*sample);
                            }
                        }
                        sum
                    };
                    for (total, sum) in totals[target..target + 4].iter_mut().zip(sum) {
                        *total += sum;
                    }
                    target += 4;
                }
            }
            let rows = last - first;
            for (x, block) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let count = columns[x] * rows;
                for (pixel, total) in block.iter_mut().zip(&totals[x * 4..x * 4 + 4]) {
                    // The sample total fits `u32`; the rounding add at the block bound does not.
                    *pixel = ((u64::from(*total) + u64::from(count) / 2) / u64::from(count)) as u8;
                }
            }
            Ok(())
        },
    )?;
    cancellation.ensure_active()?;
    Ok(SourcePlane::Materialized {
        pixels,
        width,
        height,
        alpha: AlphaRepresentation::Opaque,
    })
}

pub(super) fn covered_box_averaged(
    plane: &SourcePlane<'_>,
    block: ReducedBlock,
    cancellation: &dyn Cancellation,
) -> Result<CoveredPlane<'static>> {
    let (horizontal, vertical) = (block.horizontal, block.vertical);
    let width = plane.width().div_ceil(horizontal);
    let height = plane.height().div_ceil(vertical);
    let samples = pixel_bytes(width, height, 8)? / 8;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(samples)
        .context("the image is too large to hold in memory; use a smaller image")?;
    pixels.resize(samples, [0u16; 4]);
    let columns: Vec<u32> = (0..width)
        .map(|x| block_columns(x, plane.width(), horizontal))
        .collect();
    for_each_row(
        &mut pixels,
        width as usize,
        Vec::<[u64; 4]>::new,
        |totals, y, row| {
            cancellation.ensure_active()?;
            if totals.len() != width as usize {
                pixel_bytes(width, 1, 32)?;
                totals
                    .try_reserve_exact(width as usize)
                    .context("the image is too large to hold in memory; use a smaller image")?;
                totals.resize(width as usize, [0; 4]);
            }
            totals.fill([0; 4]);
            let first = y as u32 * vertical;
            let last = (first + vertical).min(plane.height());
            for source_y in first..last {
                cancellation.ensure_active()?;
                for (x, block) in plane
                    .row(source_y)
                    .as_chunks::<4>()
                    .0
                    .chunks(horizontal as usize)
                    .enumerate()
                {
                    for pixel in block {
                        for (total, sample) in totals[x].iter_mut().zip(premultiply(pixel)) {
                            *total += u64::from(sample);
                        }
                    }
                }
            }
            for (x, pixel) in row.iter_mut().enumerate() {
                let count = u64::from(columns[x]) * u64::from(last - first);
                // 65025 * MAX_BLOCK_PIXELS plus rounding fits `u64`, not `u32`.
                for (channel, total) in pixel.iter_mut().zip(totals[x]) {
                    *channel = ((total + count / 2) / count) as u16;
                }
            }
            Ok(())
        },
    )?;
    cancellation.ensure_active()?;
    Ok(CoveredPlane::Products {
        pixels,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::test_support::{active, plane};

    #[test]
    fn box_reduction_observes_cancellation_within_rows() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let samples = [255u8; 16];
        let source = plane(&samples, 1, 4, AlphaRepresentation::Opaque);
        let checks = AtomicUsize::new(0);
        let cancellation = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let error = box_averaged(
            &source,
            ReducedBlock {
                horizontal: 1,
                vertical: 4,
            },
            &cancellation,
        )
        .err()
        .expect("cancel before completing one reduced row");
        assert!(error.is::<crate::ImageCancelled>());
    }

    /// The box stage runs only where the references run it: past the gap a kernel alone stops
    /// serving well, and never on an upscale.
    #[test]
    fn box_stage_starts_past_the_gap() {
        let factor = |input, output| reduced_factor(input, output, REDUCE_GAP);
        assert_eq!(
            factor(6000, 1920),
            1,
            "3.1x shrink stays with the kernel alone"
        );
        assert_eq!(factor(6000, 640), 3, "9.4x shrink averages three at a time");
        assert_eq!(factor(6000, 200), 10, "30x shrink averages ten at a time");
        assert_eq!(factor(256, 128), 1, "a 2x shrink averages nothing");
        assert_eq!(factor(100, 400), 1, "an upscale averages nothing");
        assert_eq!(factor(64, 64), 1, "an identity resize averages nothing");
    }

    #[test]
    fn box_edges_average_existing_samples() {
        let source: Vec<u8> = (0..15)
            .flat_map(|index| [index * 10, index * 10, index * 10, 255])
            .collect();
        let source = plane(&source, 5, 3, AlphaRepresentation::Opaque);
        let reduced = box_averaged(
            &source,
            ReducedBlock {
                horizontal: 2,
                vertical: 2,
            },
            &active,
        )
        .unwrap();
        for (y, expected) in [[30, 50, 65], [105, 125, 140]].into_iter().enumerate() {
            let expected: Vec<u8> = expected
                .into_iter()
                .flat_map(|value| [value, value, value, 255])
                .collect();
            assert_eq!(reduced.row(y as u32), expected);
        }
    }

    #[test]
    fn box_sums_preserve_channel_totals() {
        let block = [
            255u8, 10, 200, 255, 100, 80, 30, 255, 90, 140, 50, 255, 15, 20, 80, 255, 2, 7, 3, 255,
        ];
        for (pixels, expected) in [
            (1, [255, 10, 200, 255]),
            (3, [445, 230, 280, 765]),
            (4, [460, 250, 360, 1020]),
            (5, [462, 257, 363, 1275]),
        ] {
            assert_eq!(
                opaque_block_sums(Level::new(), &block[..pixels * 4]),
                expected,
                "{pixels} pixels"
            );
        }
    }

    /// Widths straddle the maximum safe `u16` vector sum and include partial edge blocks.
    #[test]
    fn wide_blocks_preserve_uniform_color() {
        let color = [255u8, 192, 128, 255];
        let source = color.repeat(512);
        let source = plane(&source, 512, 1, AlphaRepresentation::Opaque);
        for horizontal in [255, 256, 260, 512] {
            let reduced = box_averaged(
                &source,
                ReducedBlock {
                    horizontal,
                    vertical: 1,
                },
                &active,
            )
            .unwrap();
            for pixel in reduced.row(0).as_chunks::<4>().0 {
                assert_eq!(*pixel, color, "horizontal {horizontal}");
            }
        }
    }

    /// A white block of `MAX_BLOCK_PIXELS` pixels, the arithmetic ceiling `ReducedBlock::of`
    /// allows: `255 · count` plus the rounding half crosses `u32`.
    #[test]
    fn largest_block_stays_white() {
        let pixels = vec![255u8; MAX_BLOCK_PIXELS as usize * 4];
        let source = plane(&pixels, MAX_BLOCK_PIXELS, 1, AlphaRepresentation::Opaque);
        let reduced = box_averaged(
            &source,
            ReducedBlock {
                horizontal: MAX_BLOCK_PIXELS,
                vertical: 1,
            },
            &active,
        )
        .unwrap();
        for pixel in reduced.row(0).as_chunks::<4>().0 {
            assert_eq!(*pixel, [255u8; 4]);
        }
    }
}
