//! The second convolution pass, writing one straight-alpha RGBA row.

use fearless_simd::prelude::*;
use fearless_simd::{Level, Simd, dispatch, i16x8, i32x4, i64x2};

use crate::resample::weights::{AxisWeights, quantize, quantize_coverage};

/// Two RGBA pixels fit in one load of eight intermediate i16 lanes.
const PIXELS_PER_BLOCK: usize = 2;

pub(super) fn write_opaque_vertical(
    samples: &[[i16; 4]],
    weights: &[i16],
    row: &mut [u8],
    level: Level,
) {
    let w = row.len() / 4;
    let lanes = samples.as_flattened();
    let (blocks, tail) = row.as_chunks_mut::<{ PIXELS_PER_BLOCK * 4 }>();
    dispatch!(level, simd => {
        for (idx, block) in blocks.iter_mut().enumerate() {
            let first = idx * PIXELS_PER_BLOCK * 4;
            block_simd(simd, &lanes[first..], weights, w * 4, block);
        }
    });
    let first = w - tail.len() / 4;
    for (off, px) in tail.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = first + off;
        write_opaque_pixel(px, opaque_taps(|tap| samples[tap * w + x], weights));
    }
}

pub(super) fn write_opaque_horizontal(samples: &[[i16; 4]], horiz: &AxisWeights, row: &mut [u8]) {
    for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (first, weights) = horiz.at(x as u32);
        let taps = &samples[first as usize..][..weights.len()];
        write_opaque_pixel(px, opaque_taps(|tap| taps[tap], weights));
    }
}

/// One block of output pixels on the selected SIMD backend.
#[inline(always)]
fn block_simd<S: Simd>(simd: S, taps: &[i16], weights: &[i16], stride: usize, block: &mut [u8]) {
    let mut lo = i32x4::splat(simd, 0);
    let mut hi = i32x4::splat(simd, 0);
    for (off, w) in weights.iter().enumerate() {
        let samples = i16x8::from_slice(simd, &taps[off * stride..][..PIXELS_PER_BLOCK * 4]);
        let (below, above) = simd.widen_i16x8(samples);
        let w = i32x4::splat(simd, i32::from(*w));
        lo += below * w;
        hi += above * w;
    }
    let mut totals = [0i32; PIXELS_PER_BLOCK * 4];
    lo.store_slice(&mut totals[..4]);
    hi.store_slice(&mut totals[4..]);
    for (lane, px) in block.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let vals = [
            quantize(totals[lane * 4]),
            quantize(totals[lane * 4 + 1]),
            quantize(totals[lane * 4 + 2]),
        ];
        write_opaque_pixel(px, vals);
    }
}

/// The alpha lane is already opaque, so the scalar path only accumulates RGB.
fn opaque_taps(tap: impl Fn(usize) -> [i16; 4], weights: &[i16]) -> [i32; 3] {
    let mut totals = [0i32; 3];
    for (off, w) in weights.iter().enumerate() {
        let sample = tap(off);
        for (total, ch) in totals.iter_mut().zip(sample) {
            *total += i32::from(ch) * i32::from(*w);
        }
    }
    totals.map(quantize)
}

fn write_opaque_pixel(px: &mut [u8; 4], vals: [i32; 3]) {
    for (ch, val) in px.iter_mut().zip(vals) {
        *ch = val.clamp(0, i32::from(u8::MAX)) as u8;
    }
    px[3] = u8::MAX;
}

pub(super) fn write_covered_vertical(
    samples: &[[i32; 4]],
    weights: &[i16],
    row: &mut [u8],
    level: Level,
) {
    let w = row.len() / 4;
    dispatch!(level, simd => {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let vals = covered_taps(simd, |tap| samples[tap * w + x], weights);
            write_covered_pixel(px, vals);
        }
    });
}

pub(super) fn write_covered_horizontal(
    samples: &[[i32; 4]],
    horiz: &AxisWeights,
    row: &mut [u8],
    level: Level,
) {
    dispatch!(level, simd => {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (first, weights) = horiz.at(x as u32);
            let taps = &samples[first as usize..][..weights.len()];
            let vals = covered_taps(simd, |tap| taps[tap], weights);
            write_covered_pixel(px, vals);
        }
    });
}

#[inline]
fn covered_taps<S: Simd>(simd: S, tap: impl Fn(usize) -> [i32; 4], weights: &[i16]) -> [i64; 4] {
    let mut lo = i64x2::splat(simd, 0);
    let mut hi = i64x2::splat(simd, 0);
    let mask = i32x4::splat(simd, 0xffff);
    for (off, w) in weights.iter().enumerate() {
        let samples = i32x4::from_slice(simd, &tap(off));
        let w = i32x4::splat(simd, i32::from(*w));
        // Signed s = (s & 65535) + ((s >> 16) << 16). Both limb products
        // fit i32; widening before reconstruction preserves the full i64 product.
        let (lo_l, lo_u) = ((samples & mask) * w).widen();
        let (hi_l, hi_u) = ((samples >> 16u32) * w).widen();
        lo += lo_l + (hi_l << 16u32);
        hi += lo_u + (hi_u << 16u32);
    }
    let lo = lo.to_array();
    let hi = hi.to_array();
    [lo[0], lo[1], hi[0], hi[1]].map(quantize_coverage)
}

/// Restore colour using signed raw coverage, then quantize coverage to RGBA8.
fn write_covered_pixel(px: &mut [u8; 4], vals: [i64; 4]) {
    let alpha = vals[3];
    if alpha > 0 {
        for ch in 0..3 {
            px[ch] = ((vals[ch] * 255 + alpha / 2) / alpha).clamp(0, 255) as u8;
        }
        px[3] = ((alpha + 127) / 255).min(255) as u8;
    } else {
        px.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_output_matches_scalar() {
        let weights: Vec<i16> = vec![3, -11, 29, 127, 8192, -4096, 1, 0, -2, 16383];
        let stride = weights.len();
        let samples: Vec<i16> = (0..stride * weights.len() + PIXELS_PER_BLOCK * 4)
            .map(|i| ((i * 37) % 251) as i16)
            .collect();

        let mut dispatched = vec![0u8; PIXELS_PER_BLOCK * 4];
        let level = Level::new();
        dispatch!(level, simd => block_simd(simd, &samples, &weights, stride, &mut dispatched));

        let mut ref_out = vec![0u8; PIXELS_PER_BLOCK * 4];
        for (x, px) in ref_out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let vals = opaque_taps(
                |off| std::array::from_fn(|ch| samples[off * stride + x * 4 + ch]),
                &weights,
            );
            write_opaque_pixel(px, vals);
        }
        assert_eq!(dispatched, ref_out);
    }

    #[test]
    fn signed_coverage_precision() {
        let weights = [32767, -32767, 8192, -8192, 1, -1, 16384, -17, 29];
        let levels = [
            Level::new(),
            #[cfg(target_arch = "x86_64")]
            Level::baseline(),
        ];
        for lanes in [
            [-1_474_282, -65_537, 65_535, 1_474_282],
            [-65_536, -1, 65_536, 65_537],
            [65_025, 32_768, -32_768, -65_535],
        ] {
            for count in 1..=weights.len() {
                let expected: [i64; 4] = std::array::from_fn(|ch| {
                    quantize_coverage(
                        weights[..count]
                            .iter()
                            .map(|w| i64::from(lanes[ch]) * i64::from(*w))
                            .sum(),
                    )
                });
                for level in levels {
                    let vals =
                        dispatch!(level, simd => covered_taps(simd, |_| lanes, &weights[..count]));
                    assert_eq!(vals, expected);
                }
            }
        }
    }
}
