//! AArch64: Neon kernels over paired 16-bit multiply-adds.

use fearless_simd::prelude::*;
use fearless_simd::{Level, Neon, i32x4, u8x16};

use super::super::ACCUMULATE_PIXELS;

/// Accumulate one window's tap products into four channel totals, reporting the taps the Neon
/// kernel covered.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs(
    window: &[u8],
    weights: &[i16],
    totals: &mut [i32; 4],
    level: Level,
) -> usize {
    if let Some(neon) = level.as_neon() {
        pairs_neon(neon, window, weights, totals);
        weights.len() / 4 * 4
    } else {
        0
    }
}

/// Accumulate four windows' tap products that share one set of weights, reporting the taps the
/// Neon kernel covered.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs_four(
    windows: [&[u8]; 4],
    weights: &[i16],
    totals: &mut [[i32; 4]; 4],
    level: Level,
) -> usize {
    if let Some(neon) = level.as_neon() {
        pairs_neon_four(neon, windows, weights, totals);
        weights.len() / 4 * 4
    } else {
        0
    }
}

/// Accumulate one tap across a tile of pixels, reporting whether the Neon kernel ran.
pub(in crate::resample::convolution::intermediate) fn accumulate_tile(
    tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
    window: &[u8],
    weight: i16,
    level: Level,
) -> bool {
    if let Some(neon) = level.as_neon() {
        accumulate_tile_neon(neon, tile, window, weight);
        true
    } else {
        false
    }
}

// Four RGBA8 taps widen into signed lanes; each multiply-add accumulates one channel.
fearless_simd::kernel!(
    #[inline]
    fn pairs_neon(neon: Neon, window: &[u8], weights: &[i16], totals: &mut [i32; 4]) {
        use core::arch::aarch64::*;
        // Four taps per iteration: sixteen window bytes widen to two lanes of (tap, tap) per
        // channel, and each widening multiply-add covers one lane of all four channels. Its four
        // multiply-adds accumulate independently, so the next iteration issues without waiting for
        // the previous sum; integer addition is associative, so the total is unchanged.
        let mut sums = [vdupq_n_s32(0); 4];
        for index in 0..weights.len() / 4 {
            let bytes: u8x16<Neon> = u8x16::from_slice(neon, &window[index * 16..][..16]);
            let pixels: uint8x16_t = bytes.into();
            for (lane, (half, offset)) in [(vget_low_u8(pixels), 0usize), (vget_high_u8(pixels), 2)]
                .into_iter()
                .enumerate()
            {
                let samples = vreinterpretq_s16_u16(vmovl_u8(half));
                let pair = vcombine_s16(
                    vdup_n_s16(weights[index * 4 + offset]),
                    vdup_n_s16(weights[index * 4 + offset + 1]),
                );
                sums[lane * 2] =
                    vmlal_s16(sums[lane * 2], vget_low_s16(samples), vget_low_s16(pair));
                sums[lane * 2 + 1] = vmlal_high_s16(sums[lane * 2 + 1], samples, pair);
            }
        }
        let accumulator = vaddq_s32(vaddq_s32(sums[0], sums[1]), vaddq_s32(sums[2], sums[3]));
        let result: i32x4<Neon> = accumulator.simd_into(neon);
        result.store_slice(&mut totals[..4]);
    }
);

// Four output samples in one pass. Every row of a horizontal pass weighs its taps identically, so
// broadcasting the weights once serves four windows and four independent accumulator pairs.
fearless_simd::kernel!(
    #[inline]
    fn pairs_neon_four(
        neon: Neon,
        windows: [&[u8]; 4],
        weights: &[i16],
        totals: &mut [[i32; 4]; 4],
    ) {
        use core::arch::aarch64::*;
        let mut sums = [[vdupq_n_s32(0); 2]; 4];
        for index in 0..weights.len() / 4 {
            let pairs = [
                vcombine_s16(
                    vdup_n_s16(weights[index * 4]),
                    vdup_n_s16(weights[index * 4 + 1]),
                ),
                vcombine_s16(
                    vdup_n_s16(weights[index * 4 + 2]),
                    vdup_n_s16(weights[index * 4 + 3]),
                ),
            ];
            for (row, window) in windows.iter().enumerate() {
                let bytes: u8x16<Neon> = u8x16::from_slice(neon, &window[index * 16..][..16]);
                let pixels: uint8x16_t = bytes.into();
                let halves = [vget_low_u8(pixels), vget_high_u8(pixels)];
                for (half, half_pixels) in halves.into_iter().enumerate() {
                    let samples = vreinterpretq_s16_u16(vmovl_u8(half_pixels));
                    sums[row][half] = vmlal_s16(
                        sums[row][half],
                        vget_low_s16(samples),
                        vget_low_s16(pairs[half]),
                    );
                    sums[row][half] = vmlal_high_s16(sums[row][half], samples, pairs[half]);
                }
            }
        }
        for (total, sum) in totals.iter_mut().zip(sums.iter()) {
            let accumulator = vaddq_s32(sum[0], sum[1]);
            let result: i32x4<Neon> = accumulator.simd_into(neon);
            result.store_slice(&mut total[..4]);
        }
    }
);

// One tile of one output row, still in registers: sixteen bytes widen into the lane domain and one
// multiply-add per pixel accumulates its channels.
fearless_simd::kernel!(
    #[inline]
    fn accumulate_tile_neon(
        neon: Neon,
        tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
        window: &[u8],
        weight: i16,
    ) {
        use core::arch::aarch64::*;
        let scale = vdup_n_s16(weight);
        let window: u8x16<Neon> = u8x16::from_slice(neon, window);
        let window: uint8x16_t = window.into();
        for (half, half_window) in [vget_low_u8(window), vget_high_u8(window)]
            .into_iter()
            .enumerate()
        {
            let samples = vreinterpretq_s16_u16(vmovl_u8(half_window));
            for (lane, channels) in [(0usize, vget_low_s16(samples)), (1, vget_high_s16(samples))] {
                let index = half * 2 + lane;
                let destination: i32x4<Neon> = i32x4::from_slice(neon, &tile[index]);
                let mut accumulator: int32x4_t = destination.into();
                accumulator = vmlal_s16(accumulator, channels, scale);
                let result: i32x4<Neon> = accumulator.simd_into(neon);
                result.store_slice(&mut tile[index]);
            }
        }
    }
);
