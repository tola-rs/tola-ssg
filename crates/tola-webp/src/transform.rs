//! The 4×4 integer transforms VP8 codes: the DCT for residual blocks and the WHT for luma DC.

use fearless_simd::{Simd, i32x4, prelude::*, u8x16, u32x4};

/// RFC 6386 §14.4 inverse-DCT constant.
const COS_PI_8_SQRT_2_MINUS_1: i32 = 20091;
/// RFC 6386 §14.4 inverse-DCT constant.
const SIN_PI_8_SQRT_2: i32 = 35468;

#[inline(always)]
pub(crate) fn forward_dct<S: Simd>(simd: S, input: &[i32; 16]) -> [i32; 16] {
    let [d0, d1, d2, d3] = i32x4::load_four_interleaved(simd, input);
    let [d0, d1, d2, d3] = forward_pass([d0, d1, d2, d3]);
    let [d0, d1, d2, d3] = transpose([d0, d1, d2, d3]);
    let rows = forward_finish(simd, [d0, d1, d2, d3]);
    let mut output = [0; 16];
    for (row, values) in rows.into_iter().enumerate() {
        values.store_slice(&mut output[row * 4..row * 4 + 4]);
    }
    output
}

/// The first forward pass over four lanes of one row's four positions.
#[inline(always)]
fn forward_pass<S: Simd>([d0, d1, d2, d3]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let a0 = d0 + d3;
    let a1 = d1 + d2;
    let a2 = d1 - d2;
    let a3 = d0 - d3;
    [
        (a0 + a1) * 8,
        (a2 * 2217 + a3 * 5352 + 1812) >> 9u32,
        (a0 - a1) * 8,
        (a3 * 2217 - a2 * 5352 + 937) >> 9u32,
    ]
}

/// The second forward pass, whose rounding differs from the first pass.
#[inline(always)]
fn forward_finish<S: Simd>(simd: S, [d0, d1, d2, d3]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let a0 = d0 + d3;
    let a1 = d1 + d2;
    let a2 = d1 - d2;
    let a3 = d0 - d3;
    let correction = a3
        .simd_eq(0)
        .select(i32x4::splat(simd, 0), i32x4::splat(simd, 1));
    [
        (a0 + a1 + 7) >> 4u32,
        ((a2 * 2217 + a3 * 5352 + 12000) >> 16u32) + correction,
        (a0 - a1 + 7) >> 4u32,
        (a3 * 2217 - a2 * 5352 + 51000) >> 16u32,
    ]
}

#[inline(always)]
pub(crate) fn inverse_dct<S: Simd>(simd: S, input: &[i32; 16]) -> [i32; 16] {
    let rows = std::array::from_fn(|row| i32x4::from_slice(simd, &input[row * 4..row * 4 + 4]));
    let columns = transpose(inverse_pass(rows));
    let rows = inverse_pass(columns).map(|column| (column + 4) >> 3u32);
    let mut output = [0; 16];
    i32x4::store_four_interleaved(rows, &mut output);
    output
}

#[inline(always)]
fn inverse_pass<S: Simd>([d0, d1, d2, d3]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let a = d0 + d2;
    let b = d0 - d2;
    let c = ((d1 * SIN_PI_8_SQRT_2) >> 16u32) - d3 - ((d3 * COS_PI_8_SQRT_2_MINUS_1) >> 16u32);
    let d = d1 + ((d1 * COS_PI_8_SQRT_2_MINUS_1) >> 16u32) + ((d3 * SIN_PI_8_SQRT_2) >> 16u32);
    [a + d, b + c, b - c, a - d]
}

/// One 4-lane vector per 4×4 coefficient position; the lane index is the subblock.
pub(crate) type BlockLanes<S> = [i32x4<S>; 16];

/// Forward-transform four 4×4 subblocks of one contiguous 8×8 plane against their
/// prediction. The plane is loaded as four 16-byte row pairs; the gather transposes each
/// row of the four subblocks into lanes, so the two transform passes need no shuffles.
///
/// The subblock lane order is raster: top-left, top-right, bottom-left, bottom-right.
#[inline(always)]
pub(crate) fn forward_dct_8x8_lanes<S: Simd>(
    simd: S,
    samples: &[u8; 64],
    prediction: &[u8; 64],
) -> BlockLanes<S> {
    let samples = plane_chunks(simd, samples);
    let prediction = plane_chunks(simd, prediction);
    let mut lanes = [i32x4::splat(simd, 0); 16];
    // One widened chunk covers two subblock rows, so widen each row pair once.
    for pair in 0..2 {
        let top = widen_16(samples[pair]);
        let bottom = widen_16(samples[2 + pair]);
        let predicted_top = widen_16(prediction[pair]);
        let predicted_bottom = widen_16(prediction[2 + pair]);
        for half in 0..2 {
            let index = half * 2;
            let columns = [
                top[index] - predicted_top[index],
                top[index + 1] - predicted_top[index + 1],
                bottom[index] - predicted_bottom[index],
                bottom[index + 1] - predicted_bottom[index + 1],
            ];
            let row = pair * 2 + half;
            lanes[row * 4..row * 4 + 4].copy_from_slice(&forward_pass(transpose(columns)));
        }
    }
    for column in 0..4 {
        let d = [
            lanes[column],
            lanes[4 + column],
            lanes[8 + column],
            lanes[12 + column],
        ];
        let values = forward_finish(simd, d);
        for row in 0..4 {
            lanes[row * 4 + column] = values[row];
        }
    }
    lanes
}

/// Inverse-transform four blocks laid out like [`forward_dct_8x8_lanes`].
#[inline(always)]
pub(crate) fn inverse_dct_lanes<S: Simd>(simd: S, input: &BlockLanes<S>) -> BlockLanes<S> {
    let mut lanes = *input;
    for column in 0..4 {
        let d = [
            lanes[column],
            lanes[4 + column],
            lanes[8 + column],
            lanes[12 + column],
        ];
        let values = inverse_pass(d);
        for row in 0..4 {
            lanes[row * 4 + column] = values[row];
        }
    }
    let rounding = i32x4::splat(simd, 4);
    for row in 0..4 {
        let offset = row * 4;
        let d = [
            lanes[offset],
            lanes[offset + 1],
            lanes[offset + 2],
            lanes[offset + 3],
        ];
        let values = inverse_pass(d).map(|value| (value + rounding) >> 3u32);
        lanes[offset..offset + 4].copy_from_slice(&values);
    }
    lanes
}

/// Unpack coefficient-position lanes into each block's four coefficient rows.
#[inline(always)]
fn block_rows<S: Simd>(lanes: &BlockLanes<S>) -> [[i32x4<S>; 4]; 4] {
    let mut rows = [[lanes[0]; 4]; 4];
    for (row, lane) in lanes.as_chunks::<4>().0.iter().enumerate() {
        let transposed = transpose([lane[0], lane[1], lane[2], lane[3]]);
        for (block, value) in transposed.into_iter().enumerate() {
            rows[block][row] = value;
        }
    }
    rows
}

/// Scatter coefficient lanes back into per-block coefficient arrays.
pub(crate) fn block_coefficients<S: Simd>(lanes: &BlockLanes<S>) -> [[i32; 16]; 4] {
    let rows = block_rows(lanes);
    std::array::from_fn(|block| {
        let mut coefficients = [0i32; 16];
        for (row, values) in rows[block].iter().enumerate() {
            values.store_slice(&mut coefficients[row * 4..row * 4 + 4]);
        }
        coefficients
    })
}

/// Scatter quantized coefficient lanes into per-block levels.
pub(crate) fn block_levels<S: Simd>(lanes: &BlockLanes<S>) -> [[i16; 16]; 4] {
    let rows = block_rows(lanes);
    std::array::from_fn(|block| {
        let [first, second, third, fourth] = rows[block];
        let mut levels = [0i16; 16];
        first.narrow(second).store_slice(&mut levels[..8]);
        third.narrow(fourth).store_slice(&mut levels[8..]);
        levels
    })
}

/// Gather per-block quantization levels into coefficient-position lanes.
pub(crate) fn levels_to_lanes<S: Simd>(simd: S, levels: &[[i16; 16]; 4]) -> BlockLanes<S> {
    std::array::from_fn(|position| i32x4::from_fn(simd, |block| i32::from(levels[block][position])))
}

/// Reconstruct four 8×8 subblocks from inverse-transform lanes and return the plane plus
/// each subblock's sum of squared errors.
pub(crate) fn reconstruct_8x8<S: Simd>(
    simd: S,
    samples: &[u8; 64],
    prediction: &[u8; 64],
    residual: &BlockLanes<S>,
) -> ([u8; 64], [u64; 4]) {
    let samples = plane_chunks(simd, samples);
    let prediction = plane_chunks(simd, prediction);
    let zero = i32x4::splat(simd, 0);
    let maximum = i32x4::splat(simd, 255);
    let mut squared_errors = [zero; 4];
    let mut pixels = [0u8; 64];
    for pair in 0..2 {
        let top_samples = widen_16(samples[pair]);
        let bottom_samples = widen_16(samples[2 + pair]);
        let top_prediction = widen_16(prediction[pair]);
        let bottom_prediction = widen_16(prediction[2 + pair]);
        for half in 0..2 {
            let index = half * 2;
            let row = pair * 2 + half;
            let source = [
                top_samples[index],
                top_samples[index + 1],
                bottom_samples[index],
                bottom_samples[index + 1],
            ];
            let predicted = [
                top_prediction[index],
                top_prediction[index + 1],
                bottom_prediction[index],
                bottom_prediction[index + 1],
            ];
            let residuals = transpose([
                residual[row * 4],
                residual[row * 4 + 1],
                residual[row * 4 + 2],
                residual[row * 4 + 3],
            ]);
            let mut rebuilt = [u32x4::splat(simd, 0); 4];
            for block in 0..4 {
                let values = (predicted[block] + residuals[block]).max(zero).min(maximum);
                let difference = source[block] - values;
                squared_errors[block] += difference * difference;
                rebuilt[block] = values.bitcast();
            }
            let mut rows = [0u8; 16];
            rebuilt[0]
                .narrow(rebuilt[1])
                .narrow(rebuilt[2].narrow(rebuilt[3]))
                .store_slice(&mut rows);
            pixels[row * 8..row * 8 + 8].copy_from_slice(&rows[..8]);
            pixels[(row + 4) * 8..(row + 4) * 8 + 8].copy_from_slice(&rows[8..]);
        }
    }
    let squared_errors = std::array::from_fn(|block| squared_errors[block].reduce_sum() as u64);
    (pixels, squared_errors)
}

#[inline(always)]
fn plane_chunks<S: Simd>(simd: S, plane: &[u8; 64]) -> [u8x16<S>; 4] {
    std::array::from_fn(|chunk| u8x16::from_slice(simd, &plane[chunk * 16..chunk * 16 + 16]))
}

#[inline(always)]
fn widen_16<S: Simd>(bytes: u8x16<S>) -> [i32x4<S>; 4] {
    let (low, high) = bytes.widen();
    let (first, second) = low.widen();
    let (third, fourth) = high.widen();
    [
        first.bitcast(),
        second.bitcast(),
        third.bitcast(),
        fourth.bitcast(),
    ]
}

#[inline(always)]
fn transpose<S: Simd>([a, b, c, d]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let ac_low = a.zip_low(c);
    let ac_high = a.zip_high(c);
    let bd_low = b.zip_low(d);
    let bd_high = b.zip_high(d);
    [
        ac_low.zip_low(bd_low),
        ac_low.zip_high(bd_low),
        ac_high.zip_low(bd_high),
        ac_high.zip_high(bd_high),
    ]
}

/// Transform the 4×4 block of luma DC coefficients.
pub(crate) fn forward_wht(input: &[i32; 16]) -> [i32; 16] {
    let mut intermediate = [0i32; 16];
    for column in 0..4 {
        let transformed = wht_pass([
            input[column],
            input[4 + column],
            input[8 + column],
            input[12 + column],
        ]);
        for row in 0..4 {
            intermediate[row * 4 + column] = transformed[row];
        }
    }

    let mut output = [0i32; 16];
    for row in 0..4 {
        let offset = row * 4;
        let transformed = wht_pass([
            intermediate[offset],
            intermediate[offset + 1],
            intermediate[offset + 2],
            intermediate[offset + 3],
        ]);
        for column in 0..4 {
            output[offset + column] = divide_by_two(transformed[column]);
        }
    }
    output
}

/// Reconstruct the 4×4 block of luma DC coefficients.
pub(crate) fn inverse_wht(input: &[i32; 16]) -> [i32; 16] {
    let mut output = [0i32; 16];

    for column in 0..4 {
        let a = input[column] + input[12 + column];
        let b = input[4 + column] + input[8 + column];
        let c = input[4 + column] - input[8 + column];
        let d = input[column] - input[12 + column];

        output[column] = a + b;
        output[4 + column] = c + d;
        output[8 + column] = a - b;
        output[12 + column] = d - c;
    }

    for row in 0..4 {
        let offset = row * 4;
        let a = output[offset] + output[offset + 3];
        let b = output[offset + 1] + output[offset + 2];
        let c = output[offset + 1] - output[offset + 2];
        let d = output[offset] - output[offset + 3];

        output[offset] = (a + b + 3) >> 3;
        output[offset + 1] = (c + d + 3) >> 3;
        output[offset + 2] = (a - b + 3) >> 3;
        output[offset + 3] = (d - c + 3) >> 3;
    }

    output
}

#[inline(always)]
pub(crate) fn reconstruct_sse<S: Simd>(
    simd: S,
    samples: &[u8; 16],
    prediction: &[u8; 16],
    residual: &[i32; 16],
) -> ([u8; 16], u64) {
    let samples = widen_samples(simd, samples);
    let prediction = widen_samples(simd, prediction);
    let mut pixels = [u32x4::splat(simd, 0); 4];
    let mut squared_errors = i32x4::splat(simd, 0);
    for chunk in 0..4 {
        let residual = i32x4::from_slice(simd, &residual[chunk * 4..chunk * 4 + 4]);
        let reconstructed = (prediction[chunk] + residual).max(0).min(255);
        let difference = samples[chunk] - reconstructed;
        squared_errors += difference * difference;
        pixels[chunk] = reconstructed.bitcast();
    }
    let mut output = [0; 16];
    // Clamping precedes narrowing; residuals retain their full i32 precision.
    pixels[0]
        .narrow(pixels[1])
        .narrow(pixels[2].narrow(pixels[3]))
        .store_slice(&mut output);
    // Each error is at most 255, so the block sum is at most 16 * 255².
    (output, squared_errors.reduce_sum() as u64)
}

#[inline(always)]
fn widen_samples<S: Simd>(simd: S, samples: &[u8; 16]) -> [i32x4<S>; 4] {
    let (low, high) = u8x16::from_slice(simd, samples).widen();
    let (first, second) = low.widen();
    let (third, fourth) = high.widen();
    [
        first.bitcast(),
        second.bitcast(),
        third.bitcast(),
        fourth.bitcast(),
    ]
}

fn wht_pass(values: [i32; 4]) -> [i32; 4] {
    let a = values[0] + values[3];
    let b = values[1] + values[2];
    let c = values[1] - values[2];
    let d = values[0] - values[3];
    [a + b, c + d, a - b, d - c]
}

fn divide_by_two(value: i32) -> i32 {
    if value < 0 {
        -((-value + 1) >> 1)
    } else {
        (value + 1) >> 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fearless_simd::{Fallback, Level, dispatch};

    // Scalar equations keep the reference independent of lane order and vector rounding.
    fn scalar_forward_dct(input: &[i32; 16]) -> [i32; 16] {
        let mut tmp = [0i32; 16];
        for row in 0..4 {
            let d0 = input[row * 4];
            let d1 = input[row * 4 + 1];
            let d2 = input[row * 4 + 2];
            let d3 = input[row * 4 + 3];
            let a0 = d0 + d3;
            let a1 = d1 + d2;
            let a2 = d1 - d2;
            let a3 = d0 - d3;
            tmp[row * 4] = (a0 + a1) * 8;
            tmp[row * 4 + 1] = (a2 * 2217 + a3 * 5352 + 1812) >> 9;
            tmp[row * 4 + 2] = (a0 - a1) * 8;
            tmp[row * 4 + 3] = (a3 * 2217 - a2 * 5352 + 937) >> 9;
        }

        let mut output = [0i32; 16];
        for column in 0..4 {
            let a0 = tmp[column] + tmp[12 + column];
            let a1 = tmp[4 + column] + tmp[8 + column];
            let a2 = tmp[4 + column] - tmp[8 + column];
            let a3 = tmp[column] - tmp[12 + column];
            output[column] = (a0 + a1 + 7) >> 4;
            output[4 + column] = ((a2 * 2217 + a3 * 5352 + 12000) >> 16) + i32::from(a3 != 0);
            output[8 + column] = (a0 - a1 + 7) >> 4;
            output[12 + column] = (a3 * 2217 - a2 * 5352 + 51000) >> 16;
        }
        output
    }

    fn scalar_inverse_dct(input: &[i32; 16]) -> [i32; 16] {
        let mut output = [0i32; 16];

        for column in 0..4 {
            let a = input[column] + input[8 + column];
            let b = input[column] - input[8 + column];
            let c = ((input[4 + column] * SIN_PI_8_SQRT_2) >> 16)
                - input[12 + column]
                - ((input[12 + column] * COS_PI_8_SQRT_2_MINUS_1) >> 16);
            let d = input[4 + column]
                + ((input[4 + column] * COS_PI_8_SQRT_2_MINUS_1) >> 16)
                + ((input[12 + column] * SIN_PI_8_SQRT_2) >> 16);

            output[column] = a + d;
            output[4 + column] = b + c;
            output[8 + column] = b - c;
            output[12 + column] = a - d;
        }

        for row in 0..4 {
            let offset = row * 4;
            let a = output[offset] + output[offset + 2];
            let b = output[offset] - output[offset + 2];
            let c = ((output[offset + 1] * SIN_PI_8_SQRT_2) >> 16)
                - output[offset + 3]
                - ((output[offset + 3] * COS_PI_8_SQRT_2_MINUS_1) >> 16);
            let d = output[offset + 1]
                + ((output[offset + 1] * COS_PI_8_SQRT_2_MINUS_1) >> 16)
                + ((output[offset + 3] * SIN_PI_8_SQRT_2) >> 16);

            output[offset] = (a + d + 4) >> 3;
            output[offset + 1] = (b + c + 4) >> 3;
            output[offset + 2] = (b - c + 4) >> 3;
            output[offset + 3] = (a - d + 4) >> 3;
        }

        output
    }

    fn forward_matches_reference<S: Simd>(simd: S) {
        for value in -255..=255 {
            let constant = [value; 16];
            assert_eq!(forward_dct(simd, &constant), scalar_forward_dct(&constant));
            for position in 0..16 {
                let mut impulse = [0; 16];
                impulse[position] = value;
                assert_eq!(forward_dct(simd, &impulse), scalar_forward_dct(&impulse));
            }
        }
        let mut sequence = 0x1234_5678u32;
        for _ in 0..10000 {
            let residual = std::array::from_fn(|_| {
                sequence ^= sequence << 13;
                sequence ^= sequence >> 17;
                sequence ^= sequence << 5;
                (sequence % 511) as i32 - 255
            });
            assert_eq!(forward_dct(simd, &residual), scalar_forward_dct(&residual));
        }
    }

    fn inverse_matches_reference<S: Simd>(simd: S) {
        let boundaries = [
            -4096, -2048, -1025, -1024, -255, -8, -7, -1, 0, 1, 7, 8, 255, 1024, 1025, 2048, 4096,
        ];
        for value in boundaries {
            let constant = [value; 16];
            assert_eq!(inverse_dct(simd, &constant), scalar_inverse_dct(&constant));
            for position in 0..16 {
                let mut impulse = [0; 16];
                impulse[position] = value;
                assert_eq!(inverse_dct(simd, &impulse), scalar_inverse_dct(&impulse));
            }
        }
        let mut sequence = 0x8765_4321u32;
        for _ in 0..10000 {
            let coefficients = std::array::from_fn(|_| {
                sequence ^= sequence << 13;
                sequence ^= sequence >> 17;
                sequence ^= sequence << 5;
                (sequence % 8193) as i32 - 4096
            });
            assert_eq!(
                inverse_dct(simd, &coefficients),
                scalar_inverse_dct(&coefficients)
            );
        }
    }

    /// Both directions, on every backend the machine offers, against the scalar equations.
    fn transforms_match_reference<S: Simd>(simd: S) {
        forward_matches_reference(simd);
        inverse_matches_reference(simd);
    }

    #[test]
    fn every_backend_matches_scalar_rounding() {
        transforms_match_reference(Fallback::new());
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        dispatch!(Level::baseline(), simd => transforms_match_reference(simd));
        dispatch!(Level::new(), simd => transforms_match_reference(simd));
    }

    #[test]
    fn dct_round_trips_small_residuals() {
        let input: [i32; 16] = [5, -3, 2, 0, -8, 4, 1, 0, 3, -2, 6, -1, 0, 1, -4, 2];
        let coefficients = forward_dct(Fallback::new(), &input);
        let reconstructed = inverse_dct(Fallback::new(), &coefficients);
        for (original, rebuilt) in input.iter().zip(reconstructed.iter()) {
            assert!((original - rebuilt).abs() <= 1, "{original} -> {rebuilt}");
        }
    }

    #[test]
    fn wht_round_trips_its_dc_grid() {
        let input: [i32; 16] = [8, -4, 2, 0, 1, 3, -2, 5, 0, -6, 4, 2, 7, 1, -1, 3];
        let transformed = forward_wht(&input);
        let rebuilt = inverse_wht(&transformed);
        for (original, output) in input.iter().zip(rebuilt.iter()) {
            assert!((original - output).abs() <= 1, "{original} -> {output}");
        }
    }
}
