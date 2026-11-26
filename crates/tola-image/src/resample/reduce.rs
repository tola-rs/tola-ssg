//! The box stage: whole-block averaging that runs before the kernel on a large shrink.

use fearless_simd::prelude::*;
use fearless_simd::{Level, dispatch, u8x16, u16x8};

use crate::cancellation::Cancellation;
use crate::rows::for_each_row;
use crate::{HOLD_IN_MEMORY, pixel_bytes, rgba_buffer};
use crate::{ImageError, ImageResult};

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
    pub(super) fn dimensions(self, w: u32, h: u32) -> (u32, u32) {
        (w.div_ceil(self.horizontal), h.div_ceil(self.vertical))
    }

    pub(super) fn of(src: (u32, u32), dst: (u32, u32)) -> Option<Self> {
        let mut block = Self {
            horizontal: reduced_factor(src.0, dst.0, REDUCE_GAP),
            vertical: reduced_factor(src.1, dst.1, REDUCE_GAP),
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

/// Sum four channels over one opaque block (<=255 px fits `u16` lanes).
fn opaque_block_sums(level: Level, block: &[u8]) -> [u32; 4] {
    let whole = block.len() / 16 * 16;
    let (vec_part, tail) = block.split_at(whole);
    let mut sums = [0u32; 4];
    dispatch!(level, simd => {
        let mut lo = u16x8::splat(simd, 0);
        let mut hi = u16x8::splat(simd, 0);
        for chunk in vec_part.as_chunks::<16>().0 {
            let (below, above) = u8x16::from_slice(simd, chunk).widen();
            lo += below;
            hi += above;
        }
        // 16-byte load holds 4 pixels: even -> lower lanes, odd -> upper lanes.
        let lanes = (lo + hi).to_array();
        for (total, (even, odd)) in sums.iter_mut().zip(lanes.iter().zip(&lanes[4..])) {
            *total = u32::from(*even) + u32::from(*odd);
        }
    });
    for px in tail.as_chunks::<4>().0 {
        for (total, sample) in sums.iter_mut().zip(px) {
            *total += u32::from(*sample);
        }
    }
    sums
}

fn block_columns(x: u32, w: u32, horiz: u32) -> u32 {
    let first = x * horiz;
    (first + horiz).min(w) - first
}

/// Edge blocks average only the source pixels that exist.
pub(super) fn box_averaged(
    plane: &SourcePlane<'_>,
    block: ReducedBlock,
    cancel: &dyn Cancellation,
) -> ImageResult<SourcePlane<'static>> {
    let (horiz, vert) = (block.horizontal, block.vertical);
    let (w, h) = block.dimensions(plane.width(), plane.height());
    let mut px = rgba_buffer(w, h)?;
    let level = Level::new();
    let cols: Vec<u32> = (0..w)
        .map(|x| block_columns(x, plane.width(), horiz))
        .collect();
    let stride = w as usize * 4;
    for_each_row(
        &mut px,
        stride,
        || vec![0u32; w as usize * 4],
        |totals, y, row| {
            cancel.ensure_active()?;
            totals.fill(0);
            let first = y as u32 * vert;
            let last = (first + vert).min(plane.height());
            for src_y in first..last {
                cancel.ensure_active()?;
                let src = plane.row(src_y);
                let mut target = 0;
                for (idx, block) in src.as_chunks::<4>().0.chunks(horiz as usize).enumerate() {
                    // More than 255 pixels would overflow the vector's `u16` sums.
                    let sum = if (4..=255).contains(&horiz) {
                        let off = idx * horiz as usize * 4;
                        opaque_block_sums(level, &src[off..][..block.len() * 4])
                    } else {
                        let mut sum = [0u32; 4];
                        for p in block {
                            for (total, sample) in sum.iter_mut().zip(p) {
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
                let count = cols[x] * rows;
                for (p, total) in block.iter_mut().zip(&totals[x * 4..x * 4 + 4]) {
                    // Sample total fits `u32`; rounding add at the block bound does not.
                    *p = ((u64::from(*total) + u64::from(count) / 2) / u64::from(count)) as u8;
                }
            }
            Ok(())
        },
    )?;
    cancel.ensure_active()?;
    Ok(SourcePlane::Materialized {
        pixels: px,
        width: w,
        height: h,
        alpha: AlphaRepresentation::Opaque,
    })
}

pub(super) fn covered_box_averaged(
    plane: &SourcePlane<'_>,
    block: ReducedBlock,
    cancel: &dyn Cancellation,
) -> ImageResult<CoveredPlane<'static>> {
    let (horiz, vert) = (block.horizontal, block.vertical);
    let (w, h) = block.dimensions(plane.width(), plane.height());
    let samples = pixel_bytes(w, h, 8)? / 8;
    let mut px = Vec::new();
    px.try_reserve_exact(samples)
        .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
    px.resize(samples, [0u16; 4]);
    let cols: Vec<u32> = (0..w)
        .map(|x| block_columns(x, plane.width(), horiz))
        .collect();
    for_each_row(
        &mut px,
        w as usize,
        Vec::<[u64; 4]>::new,
        |totals, y, row| {
            cancel.ensure_active()?;
            if totals.len() != w as usize {
                pixel_bytes(w, 1, 32)?;
                totals
                    .try_reserve_exact(w as usize)
                    .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
                totals.resize(w as usize, [0; 4]);
            }
            totals.fill([0; 4]);
            let first = y as u32 * vert;
            let last = (first + vert).min(plane.height());
            for src_y in first..last {
                cancel.ensure_active()?;
                for (x, block) in plane
                    .row(src_y)
                    .as_chunks::<4>()
                    .0
                    .chunks(horiz as usize)
                    .enumerate()
                {
                    for p in block {
                        for (total, sample) in totals[x].iter_mut().zip(premultiply(p)) {
                            *total += u64::from(sample);
                        }
                    }
                }
            }
            for (x, p) in row.iter_mut().enumerate() {
                let count = u64::from(cols[x]) * u64::from(last - first);
                // 65025 * MAX_BLOCK_PIXELS plus rounding fits `u64`, not `u32`.
                for (ch, total) in p.iter_mut().zip(totals[x]) {
                    *ch = ((total + count / 2) / count) as u16;
                }
            }
            Ok(())
        },
    )?;
    cancel.ensure_active()?;
    Ok(CoveredPlane::Products {
        pixels: px,
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::resample::tests::test_support::plane;

    #[test]
    fn box_reduce_cancel() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let samples = [255u8; 16];
        let src = plane(&samples, 1, 4, AlphaRepresentation::Opaque);
        let checks = AtomicUsize::new(0);
        let cancel = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let err = box_averaged(
            &src,
            ReducedBlock {
                horizontal: 1,
                vertical: 4,
            },
            &cancel,
        )
        .err()
        .expect("cancel before completing one reduced row");
        assert!(matches!(err, ImageError::Cancelled(_)));
    }

    #[test]
    fn box_starts_past_gap() {
        let factor = |i, o| reduced_factor(i, o, REDUCE_GAP);
        assert_eq!(factor(6000, 1920), 1);
        assert_eq!(factor(6000, 640), 3);
        assert_eq!(factor(6000, 200), 10);
        assert_eq!(factor(256, 128), 1);
        assert_eq!(factor(100, 400), 1);
        assert_eq!(factor(64, 64), 1);
    }

    #[test]
    fn box_edges_partial() {
        let src: Vec<u8> = (0..15)
            .flat_map(|i| [i * 10, i * 10, i * 10, 255])
            .collect();
        let src = plane(&src, 5, 3, AlphaRepresentation::Opaque);
        let reduced = box_averaged(
            &src,
            ReducedBlock {
                horizontal: 2,
                vertical: 2,
            },
            &NO_CANCELLATION,
        )
        .unwrap();
        for (y, expected) in [[30, 50, 65], [105, 125, 140]].into_iter().enumerate() {
            let expected: Vec<u8> = expected.into_iter().flat_map(|v| [v, v, v, 255]).collect();
            assert_eq!(reduced.row(y as u32), expected);
        }
    }

    #[test]
    fn box_sums_channel_totals() {
        let block = [
            255u8, 10, 200, 255, 100, 80, 30, 255, 90, 140, 50, 255, 15, 20, 80, 255, 2, 7, 3, 255,
        ];
        for (n, expected) in [
            (1, [255, 10, 200, 255]),
            (3, [445, 230, 280, 765]),
            (4, [460, 250, 360, 1020]),
            (5, [462, 257, 363, 1275]),
        ] {
            assert_eq!(
                opaque_block_sums(Level::new(), &block[..n * 4]),
                expected,
                "{n} px"
            );
        }
    }

    #[test]
    fn wide_blocks_uniform_color() {
        let color = [255u8, 192, 128, 255];
        let src = color.repeat(512);
        let src = plane(&src, 512, 1, AlphaRepresentation::Opaque);
        for horiz in [255, 256, 260, 512] {
            let reduced = box_averaged(
                &src,
                ReducedBlock {
                    horizontal: horiz,
                    vertical: 1,
                },
                &NO_CANCELLATION,
            )
            .unwrap();
            for p in reduced.row(0).as_chunks::<4>().0 {
                assert_eq!(*p, color, "horizontal {horiz}");
            }
        }
    }

    /// `255 * MAX_BLOCK_PIXELS + count/2` crosses `u32`.
    #[test]
    fn max_block_stays_white() {
        let px = vec![255u8; MAX_BLOCK_PIXELS as usize * 4];
        let src = plane(&px, MAX_BLOCK_PIXELS, 1, AlphaRepresentation::Opaque);
        let reduced = box_averaged(
            &src,
            ReducedBlock {
                horizontal: MAX_BLOCK_PIXELS,
                vertical: 1,
            },
            &NO_CANCELLATION,
        )
        .unwrap();
        for p in reduced.row(0).as_chunks::<4>().0 {
            assert_eq!(*p, [255u8; 4]);
        }
    }
}
