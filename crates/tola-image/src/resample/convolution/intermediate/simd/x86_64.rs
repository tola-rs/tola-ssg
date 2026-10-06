//! x86-64: paired multiply-add kernels over SSE2, SSE4.2, and AVX2.

use fearless_simd::prelude::*;
use fearless_simd::{Avx2, Level, Sse2, Sse4_2, i32x4, i32x8, u8x16, u8x32};

use super::super::ACCUMULATE_PIXELS;

/// Accumulate one window's tap products into four channel totals, reporting the taps the widest
/// x86 kernel covered.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs(
    window: &[u8],
    weights: &[i16],
    totals: &mut [i32; 4],
    level: Level,
) -> usize {
    let mut covered = 0;
    if let Some(avx2) = level.as_avx2() {
        pairs_avx2(avx2, window, weights, totals);
        covered = weights.len() / 8 * 8;
    }
    if covered == 0 {
        if let Some(sse4) = level.as_sse4_2() {
            pairs_sse4(sse4, window, weights, totals);
            covered = weights.len() / 4 * 4;
        } else if let Some(sse2) = level.as_sse2() {
            pairs_sse2(sse2, window, weights, totals);
            covered = weights.len() / 4 * 4;
        }
    }
    covered
}

/// Accumulate four windows' tap products that share one set of weights, reporting the taps the
/// widest x86 kernel covered.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs_four(
    windows: [&[u8]; 4],
    weights: &[i16],
    totals: &mut [[i32; 4]; 4],
    level: Level,
) -> usize {
    let mut covered = 0;
    if let Some(avx2) = level.as_avx2() {
        pairs_avx2_four(avx2, windows, weights, totals);
        covered = weights.len() / 8 * 8;
    }
    if covered == 0
        && let Some(sse2) = level.as_sse2()
    {
        pairs_sse2_four(sse2, windows, weights, totals);
        covered = weights.len() / 4 * 4;
    }
    covered
}

/// Accumulate one tap across a tile of pixels, reporting whether an x86 kernel ran.
pub(in crate::resample::convolution::intermediate) fn accumulate_tile(
    tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
    window: &[u8],
    weight: i16,
    level: Level,
) -> bool {
    if let Some(avx2) = level.as_avx2() {
        accumulate_tile_avx2(avx2, tile, window, weight);
        true
    } else if let Some(sse4) = level.as_sse4_2() {
        accumulate_tile_sse4(sse4, tile, window, weight);
        true
    } else {
        false
    }
}

/// Two weights as one 32-bit lane pair, for the paired multiply-add.
#[inline]
fn weight_pair(first: i16, second: i16) -> i32 {
    let low = (first as u16).to_ne_bytes();
    let high = (second as u16).to_ne_bytes();
    i32::from_ne_bytes([low[0], low[1], high[0], high[1]])
}

// The pairing through SSE2's paired multiply-add, which sums the two products of each 32-bit
// lane: the samples interleave so each lane holds one channel's two taps.
fearless_simd::kernel!(
    #[inline]
    fn pairs_sse2(sse2: Sse2, window: &[u8], weights: &[i16], totals: &mut [i32; 4]) {
        use core::arch::x86_64::*;
        // Two taps per paired multiply-add, two multiply-adds per half: each accumulates on its
        // own chain so the next iteration issues without waiting for the previous sum. Integer
        // addition is associative, so the total is unchanged.
        let mut sums = [_mm_setzero_si128(); 2];
        for index in 0..weights.len() / 4 {
            let bytes: u8x16<Sse2> = u8x16::from_slice(sse2, &window[index * 16..][..16]);
            let pixels: __m128i = bytes.into();
            for (lane, (half, offset)) in [(pixels, 0usize), (_mm_srli_si128(pixels, 8), 2)]
                .into_iter()
                .enumerate()
            {
                let first = _mm_cvtsi32_si128(_mm_cvtsi128_si32(half));
                let second = _mm_cvtsi32_si128(_mm_cvtsi128_si32(_mm_srli_si128(half, 4)));
                let interleaved = _mm_unpacklo_epi8(first, second);
                let samples = _mm_unpacklo_epi8(interleaved, _mm_setzero_si128());
                let pair =
                    weight_pair(weights[index * 4 + offset], weights[index * 4 + offset + 1]);
                sums[lane] =
                    _mm_add_epi32(sums[lane], _mm_madd_epi16(samples, _mm_set1_epi32(pair)));
            }
        }
        let accumulator = _mm_add_epi32(sums[0], sums[1]);
        let result: i32x4<Sse2> = accumulator.simd_into(sse2);
        result.store_slice(&mut totals[..4]);
    }
);

// The four-row shape through SSE2's paired multiply-add.
fearless_simd::kernel!(
    #[inline]
    fn pairs_sse2_four(
        sse2: Sse2,
        windows: [&[u8]; 4],
        weights: &[i16],
        totals: &mut [[i32; 4]; 4],
    ) {
        use core::arch::x86_64::*;
        let mut sums = [[_mm_setzero_si128(); 2]; 4];
        for index in 0..weights.len() / 4 {
            let pairs = [
                weight_pair(weights[index * 4], weights[index * 4 + 1]),
                weight_pair(weights[index * 4 + 2], weights[index * 4 + 3]),
            ];
            for (row, window) in windows.iter().enumerate() {
                let bytes: u8x16<Sse2> = u8x16::from_slice(sse2, &window[index * 16..][..16]);
                let pixels: __m128i = bytes.into();
                let halves = [pixels, _mm_srli_si128(pixels, 8)];
                for (half, half_pixels) in halves.into_iter().enumerate() {
                    let first = _mm_cvtsi32_si128(_mm_cvtsi128_si32(half_pixels));
                    let second =
                        _mm_cvtsi32_si128(_mm_cvtsi128_si32(_mm_srli_si128(half_pixels, 4)));
                    let interleaved = _mm_unpacklo_epi8(first, second);
                    let samples = _mm_unpacklo_epi8(interleaved, _mm_setzero_si128());
                    sums[row][half] = _mm_add_epi32(
                        sums[row][half],
                        _mm_madd_epi16(samples, _mm_set1_epi32(pairs[half])),
                    );
                }
            }
        }
        for (total, sum) in totals.iter_mut().zip(sums.iter()) {
            let accumulator = _mm_add_epi32(sum[0], sum[1]);
            let result: i32x4<Sse2> = accumulator.simd_into(sse2);
            result.store_slice(&mut total[..4]);
        }
    }
);

/// The two lane-relative gathers one 32-byte window needs, as taps (0,1) then taps (2,3). A byte
/// with its top bit set reads as zero.
const GATHER_MASKS: [[u8; 32]; 2] = [
    [
        0, 4, 1, 5, 2, 6, 3, 7, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0, 4, 1, 5, 2, 6,
        3, 7, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80,
    ],
    [
        8, 12, 9, 13, 10, 14, 11, 15, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 8, 12, 9, 13,
        10, 14, 11, 15, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80,
    ],
];

// The same pairing through one byte shuffle: gathering two taps into the pair layout costs a
// shuffle and a widening unpack, where the SSE2 shape extracts and rejoins them in four steps.
fearless_simd::kernel!(
    #[inline]
    fn pairs_sse4(sse4: Sse4_2, window: &[u8], weights: &[i16], totals: &mut [i32; 4]) {
        use core::arch::x86_64::*;
        // A masked byte reads as zero, so the gathered half fills the lanes the unpack widens.
        let masks = [
            _mm_setr_epi8(0, 4, 1, 5, 2, 6, 3, 7, -1, -1, -1, -1, -1, -1, -1, -1),
            _mm_setr_epi8(8, 12, 9, 13, 10, 14, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1),
        ];
        let mut sums = [_mm_setzero_si128(); 2];
        for index in 0..weights.len() / 4 {
            let bytes: u8x16<Sse4_2> = u8x16::from_slice(sse4, &window[index * 16..][..16]);
            let pixels: __m128i = bytes.into();
            for (lane, (mask, offset)) in
                [(masks[0], 0usize), (masks[1], 2)].into_iter().enumerate()
            {
                let gathered = _mm_shuffle_epi8(pixels, mask);
                let samples = _mm_unpacklo_epi8(gathered, _mm_setzero_si128());
                let pair =
                    weight_pair(weights[index * 4 + offset], weights[index * 4 + offset + 1]);
                sums[lane] =
                    _mm_add_epi32(sums[lane], _mm_madd_epi16(samples, _mm_set1_epi32(pair)));
            }
        }
        let accumulator = _mm_add_epi32(sums[0], sums[1]);
        let result: i32x4<Sse4_2> = accumulator.simd_into(sse4);
        result.store_slice(&mut totals[..4]);
    }
);

// Eight taps per iteration through 256-bit paired multiply-adds: each 128-bit lane of the sum
// gathers one tap pair, and the two lanes carry pairs four taps apart in the same window.
fearless_simd::kernel!(
    #[inline]
    fn pairs_avx2(avx2: Avx2, window: &[u8], weights: &[i16], totals: &mut [i32; 4]) {
        use core::arch::x86_64::*;
        let masks = [
            u8x32::<Avx2>::from_slice(avx2, &GATHER_MASKS[0]),
            u8x32::<Avx2>::from_slice(avx2, &GATHER_MASKS[1]),
        ];
        let masks = [masks[0].into(), masks[1].into()];
        let mut sums = [_mm256_setzero_si256(); 2];
        for index in 0..weights.len() / 8 {
            let bytes: u8x32<Avx2> = u8x32::from_slice(avx2, &window[index * 32..][..32]);
            let pixels: __m256i = bytes.into();
            let base = index * 8;
            let pairs = [
                _mm256_set_m128i(
                    _mm_set1_epi32(weight_pair(weights[base + 4], weights[base + 5])),
                    _mm_set1_epi32(weight_pair(weights[base], weights[base + 1])),
                ),
                _mm256_set_m128i(
                    _mm_set1_epi32(weight_pair(weights[base + 6], weights[base + 7])),
                    _mm_set1_epi32(weight_pair(weights[base + 2], weights[base + 3])),
                ),
            ];
            for (lane, mask) in masks.into_iter().enumerate() {
                let gathered = _mm256_shuffle_epi8(pixels, mask);
                let samples = _mm256_unpacklo_epi8(gathered, _mm256_setzero_si256());
                sums[lane] = _mm256_add_epi32(sums[lane], _mm256_madd_epi16(samples, pairs[lane]));
            }
        }
        let accumulator = _mm256_add_epi32(sums[0], sums[1]);
        let halves = _mm_add_epi32(
            _mm256_castsi256_si128(accumulator),
            _mm256_extracti128_si256(accumulator, 1),
        );
        let result: i32x4<Avx2> = halves.simd_into(avx2);
        result.store_slice(&mut totals[..4]);
    }
);

// The same eight-tap shape over four windows: the weight registers are built once per iteration
// and shared by every row.
fearless_simd::kernel!(
    #[inline]
    fn pairs_avx2_four(
        avx2: Avx2,
        windows: [&[u8]; 4],
        weights: &[i16],
        totals: &mut [[i32; 4]; 4],
    ) {
        use core::arch::x86_64::*;
        let masks = [
            u8x32::<Avx2>::from_slice(avx2, &GATHER_MASKS[0]),
            u8x32::<Avx2>::from_slice(avx2, &GATHER_MASKS[1]),
        ];
        let masks = [masks[0].into(), masks[1].into()];
        let mut sums = [[_mm256_setzero_si256(); 2]; 4];
        for index in 0..weights.len() / 8 {
            let base = index * 8;
            let pairs = [
                _mm256_set_m128i(
                    _mm_set1_epi32(weight_pair(weights[base + 4], weights[base + 5])),
                    _mm_set1_epi32(weight_pair(weights[base], weights[base + 1])),
                ),
                _mm256_set_m128i(
                    _mm_set1_epi32(weight_pair(weights[base + 6], weights[base + 7])),
                    _mm_set1_epi32(weight_pair(weights[base + 2], weights[base + 3])),
                ),
            ];
            for (row, window) in windows.iter().enumerate() {
                let bytes: u8x32<Avx2> = u8x32::from_slice(avx2, &window[index * 32..][..32]);
                let pixels: __m256i = bytes.into();
                for (lane, mask) in masks.into_iter().enumerate() {
                    let gathered = _mm256_shuffle_epi8(pixels, mask);
                    let samples = _mm256_unpacklo_epi8(gathered, _mm256_setzero_si256());
                    sums[row][lane] =
                        _mm256_add_epi32(sums[row][lane], _mm256_madd_epi16(samples, pairs[lane]));
                }
            }
        }
        for (total, sum) in totals.iter_mut().zip(sums) {
            let accumulator = _mm256_add_epi32(sum[0], sum[1]);
            let halves = _mm_add_epi32(
                _mm256_castsi256_si128(accumulator),
                _mm256_extracti128_si256(accumulator, 1),
            );
            let folded: i32x4<Avx2> = halves.simd_into(avx2);
            folded.store_slice(&mut total[..4]);
        }
    }
);

// Two pixels per multiply-add where SSE4.1 widens one: a 64-bit load carries a pixel pair.
fearless_simd::kernel!(
    #[inline]
    fn accumulate_tile_avx2(
        avx2: Avx2,
        tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
        window: &[u8],
        weight: i16,
    ) {
        use core::arch::x86_64::*;
        let scale = _mm256_set1_epi32(i32::from(weight));
        for (index, pair) in tile.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let pair_bytes: [u8; 8] = window[index * 8..][..8].try_into().expect("a pixel pair");
            let samples = _mm256_cvtepu8_epi32(_mm_cvtsi64_si128(i64::from_le_bytes(pair_bytes)));
            let accumulated: i32x8<Avx2> = _mm256_add_epi32(
                _mm256_mullo_epi32(samples, scale),
                <i32x8<Avx2>>::from_slice(avx2, pair.as_flattened()).into(),
            )
            .simd_into(avx2);
            accumulated.store_slice(pair.as_flattened_mut());
        }
    }
);

// A tile on a target without a lane-parallel 16-bit multiply: a pixel widens to four
// 32-bit lanes through SSE4.1, and one multiply-add accumulates its channels.
fearless_simd::kernel!(
    #[inline]
    fn accumulate_tile_sse4(
        sse4: Sse4_2,
        tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
        window: &[u8],
        weight: i16,
    ) {
        use core::arch::x86_64::*;
        let scale = _mm_set1_epi32(i32::from(weight));
        for (index, destination) in tile.iter_mut().enumerate() {
            let pixel = &window[index * 4..][..4];
            let bytes = i32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
            let samples = _mm_cvtepu8_epi32(_mm_cvtsi32_si128(bytes));
            let accumulated: i32x4<Sse4_2> = _mm_add_epi32(
                _mm_mullo_epi32(samples, scale),
                <i32x4<Sse4_2>>::from_slice(sse4, destination).into(),
            )
            .simd_into(sse4);
            accumulated.store_slice(destination);
        }
    }
);
