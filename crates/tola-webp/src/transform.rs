//! VP8 4×4 integer DCT and luma-DC WHT transforms.

use fearless_simd::{Simd, i32x4, prelude::*, u8x16, u32x4};

const COS_PI_8_SQRT_2_MINUS_1: i32 = 20091;
const SIN_PI_8_SQRT_2: i32 = 35468;

#[inline(always)]
pub(crate) fn forward_dct<S: Simd>(simd: S, src: &[i32; 16]) -> [i32; 16] {
    let [d0, d1, d2, d3] = i32x4::load_four_interleaved(simd, src);
    let [d0, d1, d2, d3] = forward_pass([d0, d1, d2, d3]);
    let [d0, d1, d2, d3] = transpose([d0, d1, d2, d3]);
    let rows = forward_finish(simd, [d0, d1, d2, d3]);
    let mut dst = [0; 16];
    for (row, vals) in rows.into_iter().enumerate() {
        vals.store_slice(&mut dst[row * 4..row * 4 + 4]);
    }
    dst
}

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

#[inline(always)]
fn forward_finish<S: Simd>(simd: S, [d0, d1, d2, d3]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let a0 = d0 + d3;
    let a1 = d1 + d2;
    let a2 = d1 - d2;
    let a3 = d0 - d3;
    let corr = a3
        .simd_eq(0)
        .select(i32x4::splat(simd, 0), i32x4::splat(simd, 1));
    [
        (a0 + a1 + 7) >> 4u32,
        ((a2 * 2217 + a3 * 5352 + 12000) >> 16u32) + corr,
        (a0 - a1 + 7) >> 4u32,
        (a3 * 2217 - a2 * 5352 + 51000) >> 16u32,
    ]
}

#[inline(always)]
pub(crate) fn inverse_dct<S: Simd>(simd: S, src: &[i32; 16]) -> [i32; 16] {
    if src[1..].iter().all(|&c| c == 0) {
        return [(src[0] + 4) >> 3; 16];
    }
    let rows = std::array::from_fn(|row| i32x4::from_slice(simd, &src[row * 4..row * 4 + 4]));
    let cols = transpose(inverse_pass(rows));
    let rows = inverse_pass(cols).map(|col| (col + 4) >> 3u32);
    let mut dst = [0; 16];
    i32x4::store_four_interleaved(rows, &mut dst);
    dst
}

#[inline(always)]
fn inverse_pass<S: Simd>([d0, d1, d2, d3]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let a = d0 + d2;
    let b = d0 - d2;
    let c = ((d1 * SIN_PI_8_SQRT_2) >> 16u32) - d3 - ((d3 * COS_PI_8_SQRT_2_MINUS_1) >> 16u32);
    let d = d1 + ((d1 * COS_PI_8_SQRT_2_MINUS_1) >> 16u32) + ((d3 * SIN_PI_8_SQRT_2) >> 16u32);
    [a + d, b + c, b - c, a - d]
}

/// One 4-lane vector per 4×4 coefficient position (lanes in raster subblock order).
pub(crate) type BlockLanes<S> = [i32x4<S>; 16];

#[inline(always)]
pub(crate) fn forward_dct_8x8_lanes<S: Simd>(
    simd: S,
    src: &[u8; 64],
    pred: &[u8; 64],
) -> BlockLanes<S> {
    let src = plane_chunks(simd, src);
    let pred = plane_chunks(simd, pred);
    let mut lanes = [i32x4::splat(simd, 0); 16];
    for pair in 0..2 {
        let top = widen_16(src[pair]);
        let bot = widen_16(src[2 + pair]);
        let pred_top = widen_16(pred[pair]);
        let pred_bot = widen_16(pred[2 + pair]);
        for half in 0..2 {
            let i = half * 2;
            let cols = [
                top[i] - pred_top[i],
                top[i + 1] - pred_top[i + 1],
                bot[i] - pred_bot[i],
                bot[i + 1] - pred_bot[i + 1],
            ];
            let row = pair * 2 + half;
            lanes[row * 4..row * 4 + 4].copy_from_slice(&forward_pass(transpose(cols)));
        }
    }
    for col in 0..4 {
        let vals = forward_finish(
            simd,
            [lanes[col], lanes[4 + col], lanes[8 + col], lanes[12 + col]],
        );
        for row in 0..4 {
            lanes[row * 4 + col] = vals[row];
        }
    }
    lanes
}

#[inline(always)]
pub(crate) fn inverse_dct_lanes<S: Simd>(simd: S, src: &BlockLanes<S>) -> BlockLanes<S> {
    let mut lanes = *src;
    for col in 0..4 {
        let vals = inverse_pass([lanes[col], lanes[4 + col], lanes[8 + col], lanes[12 + col]]);
        for row in 0..4 {
            lanes[row * 4 + col] = vals[row];
        }
    }
    let round = i32x4::splat(simd, 4);
    for row in 0..4 {
        let off = row * 4;
        let vals = inverse_pass([lanes[off], lanes[off + 1], lanes[off + 2], lanes[off + 3]])
            .map(|v| (v + round) >> 3u32);
        lanes[off..off + 4].copy_from_slice(&vals);
    }
    lanes
}

#[inline(always)]
fn block_rows<S: Simd>(lanes: &BlockLanes<S>) -> [[i32x4<S>; 4]; 4] {
    let mut rows = [[lanes[0]; 4]; 4];
    for (row, lane) in lanes.as_chunks::<4>().0.iter().enumerate() {
        let t = transpose([lane[0], lane[1], lane[2], lane[3]]);
        for (b, v) in t.into_iter().enumerate() {
            rows[b][row] = v;
        }
    }
    rows
}

pub(crate) fn block_coefficients<S: Simd>(lanes: &BlockLanes<S>) -> [[i32; 16]; 4] {
    let rows = block_rows(lanes);
    std::array::from_fn(|b| {
        let mut coeff = [0i32; 16];
        for (row, vals) in rows[b].iter().enumerate() {
            vals.store_slice(&mut coeff[row * 4..row * 4 + 4]);
        }
        coeff
    })
}

pub(crate) fn block_levels<S: Simd>(lanes: &BlockLanes<S>) -> [[i16; 16]; 4] {
    let rows = block_rows(lanes);
    std::array::from_fn(|b| {
        let [r0, r1, r2, r3] = rows[b];
        let mut levels = [0i16; 16];
        r0.narrow(r1).store_slice(&mut levels[..8]);
        r2.narrow(r3).store_slice(&mut levels[8..]);
        levels
    })
}

pub(crate) fn levels_to_lanes<S: Simd>(simd: S, levels: &[[i16; 16]; 4]) -> BlockLanes<S> {
    std::array::from_fn(|pos| i32x4::from_fn(simd, |b| i32::from(levels[b][pos])))
}

pub(crate) fn reconstruct_8x8<S: Simd>(
    simd: S,
    src: &[u8; 64],
    pred: &[u8; 64],
    res: &BlockLanes<S>,
) -> ([u8; 64], [u64; 4]) {
    let src = plane_chunks(simd, src);
    let pred = plane_chunks(simd, pred);
    let zero = i32x4::splat(simd, 0);
    let max = i32x4::splat(simd, 255);
    let mut sse = [zero; 4];
    let mut dst = [0u8; 64];
    for pair in 0..2 {
        let top_src = widen_16(src[pair]);
        let bot_src = widen_16(src[2 + pair]);
        let top_pred = widen_16(pred[pair]);
        let bot_pred = widen_16(pred[2 + pair]);
        for half in 0..2 {
            let i = half * 2;
            let row = pair * 2 + half;
            let s = [top_src[i], top_src[i + 1], bot_src[i], bot_src[i + 1]];
            let p = [top_pred[i], top_pred[i + 1], bot_pred[i], bot_pred[i + 1]];
            let r = transpose([
                res[row * 4],
                res[row * 4 + 1],
                res[row * 4 + 2],
                res[row * 4 + 3],
            ]);
            let mut recon = [u32x4::splat(simd, 0); 4];
            for b in 0..4 {
                let val = (p[b] + r[b]).max(zero).min(max);
                let diff = s[b] - val;
                sse[b] += diff * diff;
                recon[b] = val.bitcast();
            }
            let mut rows = [0u8; 16];
            recon[0]
                .narrow(recon[1])
                .narrow(recon[2].narrow(recon[3]))
                .store_slice(&mut rows);
            dst[row * 8..row * 8 + 8].copy_from_slice(&rows[..8]);
            dst[(row + 4) * 8..(row + 4) * 8 + 8].copy_from_slice(&rows[8..]);
        }
    }
    let sse = std::array::from_fn(|b| sse[b].reduce_sum() as u64);
    (dst, sse)
}

#[inline(always)]
fn plane_chunks<S: Simd>(simd: S, plane: &[u8; 64]) -> [u8x16<S>; 4] {
    std::array::from_fn(|i| u8x16::from_slice(simd, &plane[i * 16..i * 16 + 16]))
}

#[inline(always)]
fn widen_16<S: Simd>(bytes: u8x16<S>) -> [i32x4<S>; 4] {
    let (lo, hi) = bytes.widen();
    let (a, b) = lo.widen();
    let (c, d) = hi.widen();
    [a.bitcast(), b.bitcast(), c.bitcast(), d.bitcast()]
}

#[inline(always)]
fn transpose<S: Simd>([a, b, c, d]: [i32x4<S>; 4]) -> [i32x4<S>; 4] {
    let ac_lo = a.zip_low(c);
    let ac_hi = a.zip_high(c);
    let bd_lo = b.zip_low(d);
    let bd_hi = b.zip_high(d);
    [
        ac_lo.zip_low(bd_lo),
        ac_lo.zip_high(bd_lo),
        ac_hi.zip_low(bd_hi),
        ac_hi.zip_high(bd_hi),
    ]
}

pub(crate) fn forward_wht(src: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for col in 0..4 {
        let t = wht_pass([src[col], src[4 + col], src[8 + col], src[12 + col]]);
        for row in 0..4 {
            tmp[row * 4 + col] = t[row];
        }
    }

    let mut dst = [0i32; 16];
    for row in 0..4 {
        let off = row * 4;
        let t = wht_pass([tmp[off], tmp[off + 1], tmp[off + 2], tmp[off + 3]]);
        for col in 0..4 {
            dst[off + col] = divide_by_two(t[col]);
        }
    }
    dst
}

pub(crate) fn inverse_wht(src: &[i32; 16]) -> [i32; 16] {
    let mut dst = [0i32; 16];

    for col in 0..4 {
        let a = src[col] + src[12 + col];
        let b = src[4 + col] + src[8 + col];
        let c = src[4 + col] - src[8 + col];
        let d = src[col] - src[12 + col];

        dst[col] = a + b;
        dst[4 + col] = c + d;
        dst[8 + col] = a - b;
        dst[12 + col] = d - c;
    }

    for row in 0..4 {
        let off = row * 4;
        let a = dst[off] + dst[off + 3];
        let b = dst[off + 1] + dst[off + 2];
        let c = dst[off + 1] - dst[off + 2];
        let d = dst[off] - dst[off + 3];

        dst[off] = (a + b + 3) >> 3;
        dst[off + 1] = (c + d + 3) >> 3;
        dst[off + 2] = (a - b + 3) >> 3;
        dst[off + 3] = (d - c + 3) >> 3;
    }

    dst
}

#[inline(always)]
pub(crate) fn reconstruct_sse<S: Simd>(
    simd: S,
    src: &[u8; 16],
    pred: &[u8; 16],
    res: &[i32; 16],
) -> ([u8; 16], u64) {
    let src = widen_16(u8x16::from_slice(simd, src));
    let pred = widen_16(u8x16::from_slice(simd, pred));
    let mut px = [u32x4::splat(simd, 0); 4];
    let mut sse = i32x4::splat(simd, 0);
    for chunk in 0..4 {
        let r = i32x4::from_slice(simd, &res[chunk * 4..chunk * 4 + 4]);
        let recon = (pred[chunk] + r).max(0).min(255);
        let diff = src[chunk] - recon;
        sse += diff * diff;
        px[chunk] = recon.bitcast();
    }
    let mut dst = [0; 16];
    px[0]
        .narrow(px[1])
        .narrow(px[2].narrow(px[3]))
        .store_slice(&mut dst);
    (dst, sse.reduce_sum() as u64)
}

fn wht_pass(v: [i32; 4]) -> [i32; 4] {
    let a = v[0] + v[3];
    let b = v[1] + v[2];
    let c = v[1] - v[2];
    let d = v[0] - v[3];
    [a + b, c + d, a - b, d - c]
}

fn divide_by_two(v: i32) -> i32 {
    if v < 0 {
        -((-v + 1) >> 1)
    } else {
        (v + 1) >> 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fearless_simd::{Fallback, Level, dispatch};

    fn scalar_forward_dct(src: &[i32; 16]) -> [i32; 16] {
        let mut tmp = [0i32; 16];
        for row in 0..4 {
            let [d0, d1, d2, d3] = [
                src[row * 4],
                src[row * 4 + 1],
                src[row * 4 + 2],
                src[row * 4 + 3],
            ];
            let a0 = d0 + d3;
            let a1 = d1 + d2;
            let a2 = d1 - d2;
            let a3 = d0 - d3;
            tmp[row * 4] = (a0 + a1) * 8;
            tmp[row * 4 + 1] = (a2 * 2217 + a3 * 5352 + 1812) >> 9;
            tmp[row * 4 + 2] = (a0 - a1) * 8;
            tmp[row * 4 + 3] = (a3 * 2217 - a2 * 5352 + 937) >> 9;
        }

        let mut dst = [0i32; 16];
        for col in 0..4 {
            let a0 = tmp[col] + tmp[12 + col];
            let a1 = tmp[4 + col] + tmp[8 + col];
            let a2 = tmp[4 + col] - tmp[8 + col];
            let a3 = tmp[col] - tmp[12 + col];
            dst[col] = (a0 + a1 + 7) >> 4;
            dst[4 + col] = ((a2 * 2217 + a3 * 5352 + 12000) >> 16) + i32::from(a3 != 0);
            dst[8 + col] = (a0 - a1 + 7) >> 4;
            dst[12 + col] = (a3 * 2217 - a2 * 5352 + 51000) >> 16;
        }
        dst
    }

    fn scalar_inverse_dct(src: &[i32; 16]) -> [i32; 16] {
        let mut dst = [0i32; 16];

        for col in 0..4 {
            let a = src[col] + src[8 + col];
            let b = src[col] - src[8 + col];
            let c = ((src[4 + col] * SIN_PI_8_SQRT_2) >> 16)
                - src[12 + col]
                - ((src[12 + col] * COS_PI_8_SQRT_2_MINUS_1) >> 16);
            let d = src[4 + col]
                + ((src[4 + col] * COS_PI_8_SQRT_2_MINUS_1) >> 16)
                + ((src[12 + col] * SIN_PI_8_SQRT_2) >> 16);

            dst[col] = a + d;
            dst[4 + col] = b + c;
            dst[8 + col] = b - c;
            dst[12 + col] = a - d;
        }

        for row in 0..4 {
            let off = row * 4;
            let a = dst[off] + dst[off + 2];
            let b = dst[off] - dst[off + 2];
            let c = ((dst[off + 1] * SIN_PI_8_SQRT_2) >> 16)
                - dst[off + 3]
                - ((dst[off + 3] * COS_PI_8_SQRT_2_MINUS_1) >> 16);
            let d = dst[off + 1]
                + ((dst[off + 1] * COS_PI_8_SQRT_2_MINUS_1) >> 16)
                + ((dst[off + 3] * SIN_PI_8_SQRT_2) >> 16);

            dst[off] = (a + d + 4) >> 3;
            dst[off + 1] = (b + c + 4) >> 3;
            dst[off + 2] = (b - c + 4) >> 3;
            dst[off + 3] = (a - d + 4) >> 3;
        }

        dst
    }

    fn forward_matches_ref<S: Simd>(simd: S) {
        for v in -255..=255 {
            let constant = [v; 16];
            assert_eq!(forward_dct(simd, &constant), scalar_forward_dct(&constant));
            for pos in 0..16 {
                let mut impulse = [0; 16];
                impulse[pos] = v;
                assert_eq!(forward_dct(simd, &impulse), scalar_forward_dct(&impulse));
            }
        }
        let mut seq = 0x1234_5678u32;
        for _ in 0..10000 {
            let res = std::array::from_fn(|_| {
                seq ^= seq << 13;
                seq ^= seq >> 17;
                seq ^= seq << 5;
                (seq % 511) as i32 - 255
            });
            assert_eq!(forward_dct(simd, &res), scalar_forward_dct(&res));
        }
    }

    fn inverse_matches_ref<S: Simd>(simd: S) {
        let bounds = [
            -4096, -2048, -1025, -1024, -255, -8, -7, -1, 0, 1, 7, 8, 255, 1024, 1025, 2048, 4096,
        ];
        for v in bounds {
            let constant = [v; 16];
            assert_eq!(inverse_dct(simd, &constant), scalar_inverse_dct(&constant));
            for pos in 0..16 {
                let mut impulse = [0; 16];
                impulse[pos] = v;
                assert_eq!(inverse_dct(simd, &impulse), scalar_inverse_dct(&impulse));
            }
        }
        let mut seq = 0x8765_4321u32;
        for _ in 0..10000 {
            let coeff = std::array::from_fn(|_| {
                seq ^= seq << 13;
                seq ^= seq >> 17;
                seq ^= seq << 5;
                (seq % 8193) as i32 - 4096
            });
            assert_eq!(inverse_dct(simd, &coeff), scalar_inverse_dct(&coeff));
        }
    }

    fn transforms_match_ref<S: Simd>(simd: S) {
        forward_matches_ref(simd);
        inverse_matches_ref(simd);
    }

    #[test]
    fn dct_simd_matches_scalar() {
        transforms_match_ref(Fallback::new());
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        dispatch!(Level::baseline(), simd => transforms_match_ref(simd));
        dispatch!(Level::new(), simd => transforms_match_ref(simd));
    }

    #[test]
    fn dct_roundtrip() {
        let src: [i32; 16] = [5, -3, 2, 0, -8, 4, 1, 0, 3, -2, 6, -1, 0, 1, -4, 2];
        let coeff = forward_dct(Fallback::new(), &src);
        let recon = inverse_dct(Fallback::new(), &coeff);
        for (a, b) in src.iter().zip(recon.iter()) {
            assert!((a - b).abs() <= 1, "{a} -> {b}");
        }
    }

    #[test]
    fn wht_roundtrip() {
        let src: [i32; 16] = [8, -4, 2, 0, 1, 3, -2, 5, 0, -6, 4, 2, 7, 1, -1, 3];
        let coeff = forward_wht(&src);
        let recon = inverse_wht(&coeff);
        for (a, b) in src.iter().zip(recon.iter()) {
            assert!((a - b).abs() <= 1, "{a} -> {b}");
        }
    }
}
