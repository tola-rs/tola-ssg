//! VP8's quantizer lookup tables and per-plane scaling.

use fearless_simd::{Simd, i32x4, prelude::*};

/// RFC 6386 §14.1: DC dequantization factors indexed by the base quantizer index.
/// VP8 quantizer levels per table.
pub(crate) const QUANTIZER_LEVELS: usize = 128;
pub(crate) const DC_QLOOKUP: [i32; QUANTIZER_LEVELS] = [
    4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17, 18, 19, 20, 20, 21, 21, 22, 22, 23,
    23, 24, 25, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44,
    45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67,
    68, 69, 70, 71, 72, 73, 74, 75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 91,
    93, 95, 96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118, 122, 124, 126, 128, 130,
    132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
];

/// RFC 6386 §14.1: AC dequantization factors indexed by the base quantizer index.
pub(crate) const AC_QLOOKUP: [i32; QUANTIZER_LEVELS] = [
    4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28,
    29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
    53, 54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 78, 80, 82, 84, 86, 88, 90, 92, 94,
    96, 98, 100, 102, 104, 106, 108, 110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140,
    143, 146, 149, 152, 155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205,
    209, 213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
];

/// The six factors one frame's quantization uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Factors {
    pub(crate) y_dc: i32,
    pub(crate) y_ac: i32,
    pub(crate) y2_dc: i32,
    pub(crate) y2_ac: i32,
    pub(crate) chroma_dc: i32,
    pub(crate) chroma_ac: i32,
}

pub(crate) struct Matrix {
    factors: [i32; 4],
    half_steps: [i32; 4],
    reciprocals: [i32; 4],
}

impl Matrix {
    pub(crate) fn new(dc: i32, ac: i32) -> Self {
        let factors = [dc, ac, ac, ac];
        Self {
            factors,
            half_steps: factors.map(|factor| factor / 2),
            reciprocals: factors.map(|factor| 65536 / factor),
        }
    }

    /// Quantize nearest levels for four independent blocks, one block per lane. Position 0
    /// uses the DC factors; every other position uses the AC factors.
    #[inline(always)]
    pub(crate) fn quantize_lanes<S: Simd>(
        &self,
        simd: S,
        coefficients: &[i32x4<S>; 16],
    ) -> [i32x4<S>; 16] {
        let zero = i32x4::splat(simd, 0);
        let one = i32x4::splat(simd, 1);
        let mut levels = [zero; 16];
        levels[0] = quantize_lane(
            coefficients[0],
            i32x4::splat(simd, self.factors[0]),
            i32x4::splat(simd, self.half_steps[0]),
            i32x4::splat(simd, self.reciprocals[0]),
            zero,
            one,
            simd,
        );
        let factor = i32x4::splat(simd, self.factors[1]);
        let half_step = i32x4::splat(simd, self.half_steps[1]);
        let reciprocal = i32x4::splat(simd, self.reciprocals[1]);
        for position in 1..16 {
            levels[position] = quantize_lane(
                coefficients[position],
                factor,
                half_step,
                reciprocal,
                zero,
                one,
                simd,
            );
        }
        levels
    }

    /// Dequantize one block per lane; position 0 carries the DC factor.
    #[inline(always)]
    pub(crate) fn dequantize_lanes<S: Simd>(
        &self,
        simd: S,
        levels: &[i32x4<S>; 16],
    ) -> [i32x4<S>; 16] {
        let dc = i32x4::splat(simd, self.factors[0]);
        let ac = i32x4::splat(simd, self.factors[1]);
        std::array::from_fn(|position| {
            if position == 0 {
                levels[0] * dc
            } else {
                levels[position] * ac
            }
        })
    }

    #[inline(always)]
    pub(crate) fn quantize<S: Simd>(&self, simd: S, coefficients: &[i32; 16]) -> [i16; 16] {
        let zero = i32x4::splat(simd, 0);
        let one = i32x4::splat(simd, 1);
        let mut factors = i32x4::from_slice(simd, &self.factors);
        let mut half_steps = i32x4::from_slice(simd, &self.half_steps);
        let mut reciprocals = i32x4::from_slice(simd, &self.reciprocals);
        let mut quantized = [zero; 4];
        for (chunk, levels) in quantized.iter_mut().enumerate() {
            let coefficients = i32x4::from_slice(simd, &coefficients[chunk * 4..chunk * 4 + 4]);
            let numerator = coefficients.abs() + half_steps;
            let estimate = (numerator * reciprocals) >> 16u32;
            // For VP8's bounded coefficients numerator < 65536, so the reciprocal misses by at most one.
            let correction = (numerator - estimate * factors)
                .simd_ge(factors)
                .select(one, zero);
            let magnitude = (estimate + correction).min(2047);
            *levels = coefficients.simd_lt(0).select(-magnitude, magnitude);
            factors = i32x4::splat(simd, self.factors[1]);
            half_steps = i32x4::splat(simd, self.half_steps[1]);
            reciprocals = i32x4::splat(simd, self.reciprocals[1]);
        }
        let mut levels = [0; 16];
        quantized[0]
            .narrow(quantized[1])
            .store_slice(&mut levels[..8]);
        quantized[2]
            .narrow(quantized[3])
            .store_slice(&mut levels[8..]);
        levels
    }
}

/// Map an author quality in `0..=100` to a VP8 base quantizer index.
///
/// Higher quality uses a finer quantizer. The rounded linear map jumps from index 8 at
/// quality 94 to index 6 at 95; index 7 is the measured rate-distortion point for quality
/// 95, where the finer index 6 spends about 8% more bytes for 0.18 dB on the photographic
/// matrix while the coarser index 7 stays below libwebp at matched PSNR.
pub(crate) fn quantizer_index(quality: u8) -> u8 {
    // The rounded linear map skips index 7; it is the measured RD point for quality 95.
    const Q95_QUANTIZER_INDEX: u8 = 7;
    if quality == 95 {
        return Q95_QUANTIZER_INDEX;
    }
    (((100 - quality.min(100)) as u32 * 127 + 50) / 100) as u8
}

pub(crate) fn factors(index: u8) -> Factors {
    let index = usize::from(index.min((QUANTIZER_LEVELS - 1) as u8));
    let dc = DC_QLOOKUP[index];
    let ac = AC_QLOOKUP[index];
    Factors {
        y_dc: dc,
        y_ac: ac,
        y2_dc: 2 * dc,
        y2_ac: (ac * 155 / 100).max(8),
        chroma_dc: dc.min(132),
        chroma_ac: ac,
    }
}

/// Nearest quantization of one coefficient position across four block lanes.
#[inline(always)]
fn quantize_lane<S: Simd>(
    coefficient: i32x4<S>,
    factor: i32x4<S>,
    half_step: i32x4<S>,
    reciprocal: i32x4<S>,
    zero: i32x4<S>,
    one: i32x4<S>,
    simd: S,
) -> i32x4<S> {
    let numerator = coefficient.abs() + half_step;
    let estimate = (numerator * reciprocal) >> 16u32;
    let correction = (numerator - estimate * factor)
        .simd_ge(factor)
        .select(one, zero);
    let magnitude = (estimate + correction).min(i32x4::splat(simd, 2047));
    coefficient.simd_lt(0).select(-magnitude, magnitude)
}

/// Quantize one 4×4 block of transform coefficients.
pub(crate) fn quantize_block(coefficients: &[i32; 16], dc: i32, ac: i32) -> [i16; 16] {
    let mut levels = [0i16; 16];
    levels[0] = quantize_one(coefficients[0], dc);
    for position in 1..16 {
        levels[position] = quantize_one(coefficients[position], ac);
    }
    levels
}

/// Dequantize one 4×4 block back into transform coefficients.
pub(crate) fn dequantize_block(levels: &[i16; 16], dc: i32, ac: i32) -> [i32; 16] {
    let mut coefficients = [0i32; 16];
    coefficients[0] = i32::from(levels[0]) * dc;
    for position in 1..16 {
        coefficients[position] = i32::from(levels[position]) * ac;
    }
    coefficients
}

fn quantize_one(coefficient: i32, factor: i32) -> i16 {
    let magnitude = coefficient.abs();
    let level = ((2 * magnitude + factor) / (2 * factor)).min(2047) as i16;
    if coefficient < 0 { -level } else { level }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fearless_simd::{Fallback, Level, dispatch};

    fn reciprocal_matches_division<S: Simd>(simd: S) {
        let mut legal_factors = [false; 441];
        for index in 0..128 {
            let factors = factors(index);
            for factor in [
                factors.y_dc,
                factors.y_ac,
                factors.y2_dc,
                factors.y2_ac,
                factors.chroma_dc,
                factors.chroma_ac,
            ] {
                legal_factors[factor as usize] = true;
            }
        }
        for (factor, legal) in legal_factors.into_iter().enumerate() {
            if !legal {
                continue;
            }
            let factor = factor as i32;
            let matrix = Matrix::new(factor, factor);
            for first in (-16_320..=16_320).step_by(16) {
                let coefficients = std::array::from_fn(|lane| (first + lane as i32).min(16_320));
                assert_eq!(
                    matrix.quantize(simd, &coefficients),
                    quantize_block(&coefficients, factor, factor)
                );
            }
        }
        for index in 0..128 {
            let factors = factors(index);
            for (dc, ac) in [
                (factors.y_dc, factors.y_ac),
                (factors.y2_dc, factors.y2_ac),
                (factors.chroma_dc, factors.chroma_ac),
            ] {
                let matrix = Matrix::new(dc, ac);
                let boundaries = [
                    0,
                    1,
                    -1,
                    16_320,
                    -16_320,
                    dc / 2,
                    -dc / 2,
                    (dc + 1) / 2,
                    -(dc + 1) / 2,
                    ac / 2,
                    -ac / 2,
                    (ac + 1) / 2,
                    -(ac + 1) / 2,
                    8190,
                    -8190,
                    8191,
                ];
                for shift in 0..16 {
                    let coefficients = std::array::from_fn(|lane| boundaries[(lane + shift) % 16]);
                    assert_eq!(
                        matrix.quantize(simd, &coefficients),
                        quantize_block(&coefficients, dc, ac)
                    );
                }
            }
        }
    }

    #[test]
    fn reciprocal_quantization_matches_division() {
        reciprocal_matches_division(Fallback::new());
        dispatch!(Level::new(), simd => reciprocal_matches_division(simd));
    }

    fn lane_quantization_matches_blocks<S: Simd>(simd: S) {
        for index in 0..128 {
            let factors = factors(index);
            for (dc, ac) in [
                (factors.y_dc, factors.y_ac),
                (factors.y2_dc, factors.y2_ac),
                (factors.chroma_dc, factors.chroma_ac),
            ] {
                let matrix = Matrix::new(dc, ac);
                let boundaries = [
                    0,
                    1,
                    -1,
                    2,
                    -2,
                    dc / 2,
                    -(dc / 2),
                    (dc + 1) / 2,
                    ac / 2,
                    -(ac / 2),
                    (ac + 1) / 2,
                    2047,
                    -2047,
                    8190,
                    -8190,
                    16_320,
                    -16_320,
                ];
                for shift in 0..16 {
                    let blocks: [[i32; 16]; 4] = std::array::from_fn(|block| {
                        std::array::from_fn(|position| {
                            boundaries[(position + shift + block * 3) % boundaries.len()]
                        })
                    });
                    let lanes = std::array::from_fn(|position| {
                        i32x4::from_fn(simd, |block| blocks[block][position])
                    });
                    let levels = matrix.quantize_lanes(simd, &lanes);
                    let dequantized = matrix.dequantize_lanes(simd, &levels);
                    for (block, coefficients) in blocks.iter().enumerate() {
                        let expected = quantize_block(coefficients, dc, ac);
                        let actual = std::array::from_fn(|position| {
                            let lane: [i32; 4] = levels[position].into();
                            lane[block] as i16
                        });
                        assert_eq!(actual, expected, "q{index} block {block} shift {shift}");
                        let expected = dequantize_block(&expected, dc, ac);
                        let actual = std::array::from_fn(|position| {
                            let lane: [i32; 4] = dequantized[position].into();
                            lane[block]
                        });
                        assert_eq!(actual, expected, "q{index} block {block} shift {shift}");
                    }
                }
            }
        }
    }

    #[test]
    fn lane_quantization_matches_block_quantizer() {
        lane_quantization_matches_blocks(Fallback::new());
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        dispatch!(Level::baseline(), simd => lane_quantization_matches_blocks(simd));
        dispatch!(Level::new(), simd => lane_quantization_matches_blocks(simd));
    }

    #[test]
    fn quantizer_tables_match_spec() {
        assert_eq!((DC_QLOOKUP[0], DC_QLOOKUP[127]), (4, 157));
        assert_eq!((AC_QLOOKUP[0], AC_QLOOKUP[127]), (4, 284));
        assert_eq!(DC_QLOOKUP.iter().sum::<i32>(), 8168);
        assert_eq!(AC_QLOOKUP.iter().sum::<i32>(), 12723);
    }

    #[test]
    fn quality_refines_quantizer_monotonically() {
        assert_eq!(quantizer_index(100), 0);
        assert_eq!(quantizer_index(0), 127);
        assert_eq!(quantizer_index(94), 8);
        assert_eq!(quantizer_index(95), 7);
        assert_eq!(quantizer_index(96), 5);
        let mut previous = 127;
        for quality in 0..=100 {
            let index = quantizer_index(quality);
            assert!(index <= previous, "q{quality} regressed");
            previous = index;
        }
    }

    #[test]
    fn per_plane_factors_scale_and_clamp() {
        let fine = factors(0);
        assert_eq!(fine.y_dc, 4);
        assert_eq!(fine.y_ac, 4);
        assert_eq!(fine.y2_dc, 8);
        assert_eq!(fine.y2_ac, 8);
        assert_eq!(fine.chroma_dc, 4);

        let coarse = factors(127);
        assert_eq!(coarse.chroma_dc, 132);
        assert!(coarse.y2_ac >= coarse.y_ac);
    }

    #[test]
    fn coefficients_round_to_nearest_steps() {
        let mut coefficients = [0; 16];
        coefficients[0] = 9;
        coefficients[1] = -7;
        let levels = quantize_block(&coefficients, 4, 4);
        let mut expected = [0; 16];
        expected[0] = 2;
        expected[1] = -2;
        assert_eq!(levels, expected);
        let rebuilt = dequantize_block(&levels, 4, 4);
        expected[0] = 8;
        expected[1] = -8;
        assert_eq!(rebuilt, expected.map(i32::from));
    }
}
