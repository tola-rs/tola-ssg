//! Turning the intermediate plane into output: one output row at a time, two pixels per block.

use fearless_simd::prelude::*;
use fearless_simd::{Level, Simd, dispatch, i16x8, i32x4, i64x2};

use crate::resample::weights::{quantize, quantize_coverage};

/// Lanes one blocked output pass covers at a time: two pixels, one channel each.
pub(super) const LANES: usize = 2;

/// Eight lanes of one output row: a tap row is one contiguous load, its weight one broadcast.
pub(super) fn block(
    level: Level,
    segment: &[i16],
    weights: &[i16],
    stride: usize,
    block: &mut [u8],
) {
    dispatch!(level, simd => block_simd(simd, segment, weights, stride, block));
}

/// One block of output lanes on the widest vectors the running CPU offers.
///
/// A tap row is one contiguous load and its weight one broadcast, so a whole vector of lanes
/// advances per instruction instead of one lane.
#[inline(always)]
fn block_simd<S: Simd>(simd: S, taps: &[i16], weights: &[i16], stride: usize, block: &mut [u8]) {
    let mut lower = i32x4::splat(simd, 0);
    let mut upper = i32x4::splat(simd, 0);
    for (offset, weight) in weights.iter().enumerate() {
        let samples = i16x8::from_slice(simd, &taps[offset * stride..][..LANES * 4]);
        let (below, above) = simd.widen_i16x8(samples);
        let weight = i32x4::splat(simd, i32::from(*weight));
        lower += below * weight;
        upper += above * weight;
    }
    let mut totals = [0i32; LANES * 4];
    lower.store_slice(&mut totals[..4]);
    upper.store_slice(&mut totals[4..]);
    for (lane, pixel) in block.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let values = [
            quantize(totals[lane * 4]),
            quantize(totals[lane * 4 + 1]),
            quantize(totals[lane * 4 + 2]),
            quantize(totals[lane * 4 + 3]),
        ];
        write_pixel(pixel, values);
    }
}

/// One output sample, reading the intermediate plane's samples.
pub(super) fn taps(taps: impl Fn(usize) -> [i16; 4], weights: &[i16]) -> [i32; 4] {
    let mut totals = [0i32; 4];
    for (offset, weight) in weights.iter().enumerate() {
        let tap = taps(offset);
        let weight = i32::from(*weight);
        for channel in 0..3 {
            totals[channel] += i32::from(tap[channel]) * weight;
        }
    }
    let mut values = [0i32; 4];
    for (value, total) in values.iter_mut().zip(totals) {
        *value = quantize(total);
    }
    values
}

pub(super) fn write_pixel(pixel: &mut [u8], values: [i32; 4]) {
    for channel in 0..3 {
        pixel[channel] = values[channel].clamp(0, u8::MAX as i32) as u8;
    }
    pixel[3] = u8::MAX;
}

pub(super) fn covered_taps(
    tap: impl Fn(usize) -> [i32; 4],
    weights: &[i16],
    level: Level,
) -> [i64; 4] {
    dispatch!(level, simd => {
        let mut lower = i64x2::splat(simd, 0);
        let mut upper = i64x2::splat(simd, 0);
        let mask = i32x4::splat(simd, 0xffff);
        for (offset, weight) in weights.iter().enumerate() {
            let samples = i32x4::from_slice(simd, &tap(offset));
            let weight = i32x4::splat(simd, i32::from(*weight));
            // Signed s = (s & 65535) + ((s >> 16) << 16). Both limb products
            // fit i32; widening before reconstruction preserves the full i64 product.
            let (low_lower, low_upper) = ((samples & mask) * weight).widen();
            let (high_lower, high_upper) = ((samples >> 16u32) * weight).widen();
            lower += low_lower + (high_lower << 16u32);
            upper += low_upper + (high_upper << 16u32);
        }
        let lower = lower.to_array();
        let upper = upper.to_array();
        [lower[0], lower[1], upper[0], upper[1]].map(quantize_coverage)
    })
}

/// Restore colour using signed raw coverage, then quantize coverage to RGBA8.
pub(super) fn write_covered_pixel(pixel: &mut [u8], values: [i64; 4]) {
    let alpha = values[3];
    if alpha > 0 {
        for channel in 0..3 {
            pixel[channel] = ((values[channel] * 255 + alpha / 2) / alpha).clamp(0, 255) as u8;
        }
        pixel[3] = ((alpha + 127) / 255).min(255) as u8;
    } else {
        pixel.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_output_matches_scalar() {
        let weights: Vec<i16> = vec![3, -11, 29, 127, 8192, -4096, 1, 0, -2, 16383];
        let stride = weights.len();
        let samples: Vec<i16> = (0..stride * weights.len() + LANES * 4)
            .map(|index| ((index * 37) % 251) as i16)
            .collect();

        let mut dispatched = vec![0u8; LANES * 4];
        block(Level::new(), &samples, &weights, stride, &mut dispatched);

        let mut reference = vec![0u8; LANES * 4];
        for pixel in 0..LANES {
            let values = taps(
                |offset| {
                    std::array::from_fn(|channel| samples[offset * stride + pixel * 4 + channel])
                },
                &weights,
            );
            write_pixel(&mut reference[pixel * 4..][..4], values);
        }
        assert_eq!(dispatched, reference);
    }

    #[test]
    fn signed_coverage_products_preserve_precision() {
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
                let expected: [i64; 4] = std::array::from_fn(|channel| {
                    quantize_coverage(
                        weights[..count]
                            .iter()
                            .map(|weight| i64::from(lanes[channel]) * i64::from(*weight))
                            .sum(),
                    )
                });
                for level in levels {
                    assert_eq!(covered_taps(|_| lanes, &weights[..count], level), expected);
                }
            }
        }
    }
}
