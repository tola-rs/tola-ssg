//! Filling the intermediate plane: whole rows of taps, weighted in groups of four.

use fearless_simd::prelude::*;
use fearless_simd::{Level, Simd, dispatch, i32x4, i64x2};

use crate::resample::weights::{AxisWeights, quantize, quantize_coverage};

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
pub(super) fn dot_window(win: &[u8], weights: &[i16], level: Level) -> [i16; 4] {
    let mut totals = [0i32; 4];
    let covered = simd::accumulate_pairs(win, weights, &mut totals, level);
    remaining_taps(&mut totals, win, weights, covered);
    finish_tile(totals)
}

/// Four output samples of the intermediate pass, sharing one set of broadcast weights.
pub(super) fn dot_window_four(windows: [&[u8]; 4], weights: &[i16], level: Level) -> [[i16; 4]; 4] {
    let mut totals = [[0i32; 4]; 4];
    let covered = simd::accumulate_pairs_four(windows, weights, &mut totals, level);
    let mut res = [[0i16; 4]; 4];
    for (r, win) in windows.iter().enumerate() {
        remaining_taps(&mut totals[r], win, weights, covered);
        res[r] = finish_tile(totals[r]);
    }
    res
}

fn remaining_taps(totals: &mut [i32; 4], win: &[u8], weights: &[i16], covered: usize) {
    for (off, w) in weights.iter().enumerate().skip(covered) {
        let tap = &win[off * 4..][..4];
        let w = i32::from(*w);
        for (total, sample) in totals.iter_mut().zip(tap) {
            *total += i32::from(*sample) * w;
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
    let w = totals.len();
    let mut tile = [[0i32; 4]; ACCUMULATE_PIXELS];
    for (off, weight) in weights.iter().enumerate() {
        let win = &tap(off)[start * 4..][..w * 4];
        let weight_i = i32::from(*weight);
        if w == ACCUMULATE_PIXELS && simd::accumulate_tile(&mut tile, win, *weight, level) {
            continue;
        }
        for (total, p) in tile[..w].iter_mut().zip(win.as_chunks::<4>().0) {
            for (ch, sample) in p.iter().enumerate() {
                total[ch] += i32::from(*sample) * weight_i;
            }
        }
    }
    for (total, acc) in totals.iter_mut().zip(tile) {
        *total = acc;
    }
}

pub(super) fn finish_tile(totals: [i32; 4]) -> [i16; 4] {
    let mut res = [0i16; 4];
    for (v, total) in res[..3].iter_mut().zip(totals) {
        *v = quantize(total) as i16;
    }
    res[3] = u8::MAX as i16;
    res
}

pub(super) fn write_covered_horizontal_rows(
    srcs: [&[[u16; 4]]; WINDOW_ROWS],
    horiz: &AxisWeights,
    rows: &mut [[i32; 4]],
    w: usize,
    level: Level,
) {
    dispatch!(level, simd => {
        if rows.len() == w * WINDOW_ROWS {
            for x in 0..w {
                let (first, weights) = horiz.at(x as u32);
                let windows = srcs.map(|row| &row[first as usize..][..weights.len()]);
                for (r, v) in covered_windows(simd, windows, weights).into_iter().enumerate() {
                    rows[r * w + x] = v;
                }
            }
        } else {
            for (row, src) in rows.chunks_exact_mut(w).zip(srcs) {
                for (x, v) in row.iter_mut().enumerate() {
                    let (first, weights) = horiz.at(x as u32);
                    let win = &src[first as usize..][..weights.len()];
                    *v = covered_windows(simd, [win], weights)[0];
                }
            }
        }
    });
}

#[inline(always)]
fn covered_windows<S: Simd, const ROWS: usize>(
    simd: S,
    windows: [&[[u16; 4]]; ROWS],
    weights: &[i16],
) -> [[i32; 4]; ROWS] {
    let mut totals = [[i64x2::splat(simd, 0); 2]; ROWS];
    for (off, w) in weights.iter().enumerate() {
        let w = i32x4::splat(simd, i32::from(*w));
        for (r, win) in windows.iter().enumerate() {
            let samples = i32x4::from_slice(simd, &win[off].map(i32::from));
            // Each product fits i32; their sum does not. Widen before adding taps.
            let (lo, hi) = (samples * w).widen();
            totals[r][0] += lo;
            totals[r][1] += hi;
        }
    }
    std::array::from_fn(|r| {
        let lo = totals[r][0].to_array();
        let hi = totals[r][1].to_array();
        [lo[0], lo[1], hi[0], hi[1]].map(|sum| quantize_coverage(sum) as i32)
    })
}

pub(super) fn write_covered_vertical_row<'a>(
    samples: &mut [[i32; 4]],
    tap: impl Fn(usize) -> &'a [[u16; 4]],
    weights: &[i16],
    level: Level,
) {
    dispatch!(level, simd => {
        for (tile, dst) in samples.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
            let first = tile * ACCUMULATE_PIXELS;
            let mut sums = [[i64x2::splat(simd, 0); 2]; ACCUMULATE_PIXELS];
            for (off, w) in weights.iter().enumerate() {
                let w = i32x4::splat(simd, i32::from(*w));
                let row = tap(off);
                for (x, sum) in sums.iter_mut().take(dst.len()).enumerate() {
                    let samples = i32x4::from_slice(simd, &row[first + x].map(i32::from));
                    let (lo, hi) = (samples * w).widen();
                    sum[0] += lo;
                    sum[1] += hi;
                }
            }
            for (sample, sum) in dst.iter_mut().zip(sums) {
                let lo = sum[0].to_array();
                let hi = sum[1].to_array();
                *sample = [lo[0], lo[1], hi[0], hi[1]].map(|sum| quantize_coverage(sum) as i32);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_kernels_match_scalar() {
        let all_w: Vec<i16> = (0..12).map(|k| (k * 977 - 4096) as i16).collect();
        let all_win: Vec<u8> = (0..all_w.len() * 4)
            .map(|i| ((i * 37) % 251) as u8)
            .collect();

        for taps in 1..=all_w.len() {
            let weights = &all_w[..taps];
            let win = &all_win[..taps * 4];
            let dispatched = dot_window(win, weights, Level::new());
            let mut totals = [0i32; 4];
            remaining_taps(&mut totals, win, weights, 0);
            assert_eq!(dispatched, finish_tile(totals), "{taps} taps");
        }
    }

    #[test]
    fn four_row_kernel_matches_scalar() {
        let all_w: Vec<i16> = (0..12).map(|k| (k * 977 - 4096) as i16).collect();
        let all_wins: Vec<u8> = (0..all_w.len() * 4 * WINDOW_ROWS)
            .map(|i| ((i * 37) % 251) as u8)
            .collect();

        for taps in 1..=all_w.len() {
            let weights = &all_w[..taps];
            let windows: [&[u8]; WINDOW_ROWS] =
                std::array::from_fn(|r| &all_wins[r * taps * 4..(r + 1) * taps * 4]);
            let shared = dot_window_four(windows, weights, Level::new());
            for (r, win) in windows.iter().enumerate() {
                let mut totals = [0i32; 4];
                remaining_taps(&mut totals, win, weights, 0);
                assert_eq!(shared[r], finish_tile(totals), "{taps} taps, row {r}");
            }
        }
    }

    #[test]
    fn tiled_sums_match_scalar() {
        let weights: Vec<i16> = vec![3, -11, 29, 127, 8192, -4096, 1];
        let px = ACCUMULATE_PIXELS + 1;
        let rows: Vec<Vec<u8>> = (0..weights.len())
            .map(|r| {
                (0..px * 4)
                    .map(|i| ((i * 31 + r * 17) % 251) as u8)
                    .collect()
            })
            .collect();
        let mut totals = vec![[0i32; 4]; px];
        for (idx, tile) in totals.chunks_mut(ACCUMULATE_PIXELS).enumerate() {
            let start = idx * ACCUMULATE_PIXELS;
            accumulate_tile(tile, |off| &rows[off], &weights, start, Level::new());
        }
        for (p, total) in totals.into_iter().enumerate() {
            let win: Vec<u8> = rows
                .iter()
                .flat_map(|row| row[p * 4..][..4].iter().copied())
                .collect();
            let mut scalar = [0i32; 4];
            remaining_taps(&mut scalar, &win, &weights, 0);
            assert_eq!(total, scalar, "pixel {p}");
        }
    }

    #[test]
    fn coverage_windows_channel_sums() {
        let weights = [32767, -32767, 8192, -4096, 16384, -7, 1, 17, -29];
        let levels = [Level::new(), Level::baseline()];
        let src: Vec<[u16; 4]> = (0..weights.len())
            .map(|i| [65025, 65025 - i as u16 * 71, i as u16, 65025])
            .collect();
        for count in 1..=weights.len() {
            let expected: [i32; 4] = std::array::from_fn(|ch| {
                quantize_coverage(
                    src[..count]
                        .iter()
                        .zip(&weights[..count])
                        .map(|(p, w)| i64::from(p[ch]) * i64::from(*w))
                        .sum(),
                ) as i32
            });
            for level in levels {
                dispatch!(level, simd => {
                    assert_eq!(
                        covered_windows(simd, [&src[..count]], &weights[..count])[0],
                        expected
                    );
                    let shared =
                        covered_windows(simd, [&src[..count]; WINDOW_ROWS], &weights[..count]);
                    assert_eq!(shared, [expected; WINDOW_ROWS]);
                });
            }
        }
    }
}
