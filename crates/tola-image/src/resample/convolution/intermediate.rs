//! Filling the intermediate plane: whole rows of taps, weighted in groups of four.

use fearless_simd::prelude::*;
use fearless_simd::{Level, Simd, dispatch, i32x4, i64x2};

use crate::resample::weights::{quantize, quantize_coverage};

mod simd;

/// Rows one window of the intermediate pass covers at a time: they share one set of broadcast
/// weights.
pub(super) const WINDOW_ROWS: usize = 4;

/// Pixels one tile of the vertical pass covers: four columns, one vector's worth at 32-bit lanes.
///
/// A tile's totals stay in registers across every tap of one output row, so a tile costs one load
/// per tap and one store for the row, instead of a load and a store per pixel per tap.
pub(super) const ACCUMULATE_PIXELS: usize = 4;

/// One output sample of the intermediate pass, reading one window of prepared samples.
///
pub(super) fn dot_window(window: &[u8], weights: &[i16], level: Level) -> [i16; 4] {
    let mut totals = [0i32; 4];
    let covered = simd::accumulate_pairs(window, weights, &mut totals, level);
    remaining_taps(&mut totals, window, weights, covered);
    finish_tile(totals)
}

/// Four output samples of the intermediate pass, sharing one set of broadcast weights.
pub(super) fn dot_window_four(windows: [&[u8]; 4], weights: &[i16], level: Level) -> [[i16; 4]; 4] {
    let mut totals = [[0i32; 4]; 4];
    let covered = simd::accumulate_pairs_four(windows, weights, &mut totals, level);
    let mut results = [[0i16; 4]; 4];
    for (row, window) in windows.iter().enumerate() {
        remaining_taps(&mut totals[row], window, weights, covered);
        results[row] = finish_tile(totals[row]);
    }
    results
}

/// Add the taps a vector kernel did not cover, for one output sample of the intermediate pass.
fn remaining_taps(totals: &mut [i32; 4], window: &[u8], weights: &[i16], covered: usize) {
    for (offset, weight) in weights.iter().enumerate().skip(covered) {
        let tap = &window[offset * 4..][..4];
        let weight = i32::from(*weight);
        for (total, sample) in totals.iter_mut().zip(tap) {
            *total += i32::from(*sample) * weight;
        }
    }
}

/// Accumulate opaque rows without materializing a list of tap references.
#[inline]
pub(super) fn accumulate_tile<'a>(
    totals: &mut [[i32; 4]],
    tap: impl Fn(usize) -> &'a [u8],
    weights: &[i16],
    start: usize,
    level: Level,
) {
    let width = totals.len();
    let mut tile = [[0i32; 4]; ACCUMULATE_PIXELS];
    for (offset, weight) in weights.iter().enumerate() {
        let window = &tap(offset)[start * 4..][..width * 4];
        let weight = i32::from(*weight);
        if width == ACCUMULATE_PIXELS
            && simd::accumulate_tile(&mut tile, window, weight as i16, level)
        {
            continue;
        }
        for (total, pixel) in tile[..width].iter_mut().zip(window.as_chunks::<4>().0) {
            for (channel, sample) in pixel.iter().enumerate() {
                total[channel] += i32::from(*sample) * weight;
            }
        }
    }
    for (total, accumulated) in totals.iter_mut().zip(tile) {
        *total = accumulated;
    }
}

pub(super) fn finish_tile(totals: [i32; 4]) -> [i16; 4] {
    let mut result = [0i16; 4];
    for (value, total) in result[..3].iter_mut().zip(totals) {
        *value = quantize(total) as i16;
    }
    result[3] = u8::MAX as i16;
    result
}

pub(super) fn covered_windows<const ROWS: usize>(
    windows: [&[[u16; 4]]; ROWS],
    weights: &[i16],
    level: Level,
) -> [[i32; 4]; ROWS] {
    dispatch!(level, simd => covered_windows_simd(simd, windows, weights))
}

#[inline(always)]
fn covered_windows_simd<S: Simd, const ROWS: usize>(
    simd: S,
    windows: [&[[u16; 4]]; ROWS],
    weights: &[i16],
) -> [[i32; 4]; ROWS] {
    let mut totals = [[i64x2::splat(simd, 0); 2]; ROWS];
    for (offset, weight) in weights.iter().enumerate() {
        let weight = i32x4::splat(simd, i32::from(*weight));
        for (row, window) in windows.iter().enumerate() {
            let samples = i32x4::from_slice(simd, &window[offset].map(i32::from));
            // Each product fits i32; their sum does not. Widen before adding taps.
            let (lower, upper) = (samples * weight).widen();
            totals[row][0] += lower;
            totals[row][1] += upper;
        }
    }
    std::array::from_fn(|row| {
        let lower = totals[row][0].to_array();
        let upper = totals[row][1].to_array();
        [lower[0], lower[1], upper[0], upper[1]].map(|sum| quantize_coverage(sum) as i32)
    })
}

pub(super) fn accumulate_covered_tile<'a>(
    totals: &mut [[i64; 4]],
    tap: impl Fn(usize) -> &'a [[u16; 4]],
    weights: &[i16],
    start: usize,
    level: Level,
) {
    dispatch!(level, simd => {
        let mut sums = [[i64x2::splat(simd, 0); 2]; ACCUMULATE_PIXELS];
        for (offset, weight) in weights.iter().enumerate() {
            let weight = i32x4::splat(simd, i32::from(*weight));
            let row = tap(offset);
            for (x, sum) in sums.iter_mut().take(totals.len()).enumerate() {
                let samples = i32x4::from_slice(simd, &row[start + x].map(i32::from));
                let (lower, upper) = (samples * weight).widen();
                sum[0] += lower;
                sum[1] += upper;
            }
        }
        for (total, sum) in totals.iter_mut().zip(sums) {
            let lower = sum[0].to_array();
            let upper = sum[1].to_array();
            *total = [lower[0], lower[1], upper[0], upper[1]];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Native kernels must match the scalar remainder for both full vectors and short windows.
    #[test]
    fn paired_kernels_match_scalar() {
        let all_weights: Vec<i16> = (0..12).map(|k| (k * 977 - 4096) as i16).collect();
        let all_window: Vec<u8> = (0..all_weights.len() * 4)
            .map(|index| ((index * 37) % 251) as u8)
            .collect();

        for taps in 1..=all_weights.len() {
            let weights = &all_weights[..taps];
            let window = &all_window[..taps * 4];
            let dispatched = dot_window(window, weights, Level::new());
            let mut totals = [0i32; 4];
            remaining_taps(&mut totals, window, weights, 0);
            assert_eq!(dispatched, finish_tile(totals), "{taps} taps");
        }
    }

    /// Four rows must retain the scalar convolution's channel values for every tap count.
    #[test]
    fn four_row_kernel_matches_separate_windows() {
        let all_weights: Vec<i16> = (0..12).map(|k| (k * 977 - 4096) as i16).collect();
        let all_windows: Vec<u8> = (0..all_weights.len() * 4 * WINDOW_ROWS)
            .map(|index| ((index * 37) % 251) as u8)
            .collect();

        for taps in 1..=all_weights.len() {
            let weights = &all_weights[..taps];
            let windows: [&[u8]; WINDOW_ROWS] =
                std::array::from_fn(|row| &all_windows[row * taps * 4..(row + 1) * taps * 4]);
            let shared = dot_window_four(windows, weights, Level::new());
            for (row, window) in windows.iter().enumerate() {
                let mut totals = [0i32; 4];
                remaining_taps(&mut totals, window, weights, 0);
                assert_eq!(shared[row], finish_tile(totals), "{taps} taps, row {row}");
            }
        }
    }

    #[test]
    fn tiled_sums_match_scalar() {
        let weights: Vec<i16> = vec![3, -11, 29, 127, 8192, -4096, 1];
        let pixels = ACCUMULATE_PIXELS + 1;
        let rows: Vec<Vec<u8>> = (0..weights.len())
            .map(|row| {
                (0..pixels * 4)
                    .map(|index| ((index * 31 + row * 17) % 251) as u8)
                    .collect()
            })
            .collect();
        let mut totals = vec![[0i32; 4]; pixels];
        for (index, tile) in totals.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
            let start = index * ACCUMULATE_PIXELS;
            accumulate_tile(tile, |offset| &rows[offset], &weights, start, Level::new());
        }
        for (pixel, total) in totals.into_iter().enumerate() {
            let window: Vec<u8> = rows
                .iter()
                .flat_map(|row| row[pixel * 4..][..4].iter().copied())
                .collect();
            let mut scalar = [0i32; 4];
            remaining_taps(&mut scalar, &window, &weights, 0);
            assert_eq!(total, scalar, "pixel {pixel}");
        }
    }

    #[test]
    fn coverage_windows_preserve_channel_sums() {
        let weights = [32767, -32767, 8192, -4096, 16384, -7, 1, 17, -29];
        let levels = [Level::new(), Level::baseline()];
        let source: Vec<[u16; 4]> = (0..weights.len())
            .map(|index| [65025, 65025 - index as u16 * 71, index as u16, 65025])
            .collect();
        for count in 1..=weights.len() {
            let expected: [i32; 4] = std::array::from_fn(|channel| {
                quantize_coverage(
                    source[..count]
                        .iter()
                        .zip(&weights[..count])
                        .map(|(pixel, weight)| i64::from(pixel[channel]) * i64::from(*weight))
                        .sum(),
                ) as i32
            });
            for level in levels {
                assert_eq!(
                    covered_windows([&source[..count]], &weights[..count], level)[0],
                    expected
                );
                let shared =
                    covered_windows([&source[..count]; WINDOW_ROWS], &weights[..count], level);
                assert_eq!(shared, [expected; WINDOW_ROWS]);
            }
        }
    }
}
