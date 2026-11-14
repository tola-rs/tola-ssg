//! A VP8L predictor image over the ALPH residual plane.

use super::repeated::{Bits, GREEN_SYMBOLS, common_prefix};
use super::{Predictor, VP8L_ALPHA, entropy_score, groups};
use crate::{Error, encoder::check_cancelled};

const MODES: [u8; 8] = [0, 1, 2, 3, 4, 11, 12, 13];
const ALL_MODES: [u8; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

/// Choose at most one outer filter using sparse first- and second-order residuals.
pub(super) fn outer(pixels: &[[u8; 4]], width: usize, height: usize) -> Option<Predictor> {
    let mut counts = [[0u32; 256]; 8];
    let mut points = 0;
    let gradient = |i: usize| {
        let value = pixels[i][3];
        value.wrapping_sub(super::gradient(
            pixels[i - 1][3],
            pixels[i - width][3],
            pixels[i - width - 1][3],
        ))
    };
    for y in (2..height).step_by(11) {
        for x in (2..width - 1).step_by(11) {
            let i = y * width + x;
            let a = pixels[i][3];
            let l = pixels[i - 1][3];
            let t = pixels[i - width][3];
            let c = pixels[i - width - 1][3];
            let g = gradient(i);
            let values = [
                a.wrapping_sub(l),
                a.wrapping_sub(t),
                a.wrapping_sub(c),
                g,
                g.wrapping_sub(gradient(i - 1)),
                g.wrapping_sub(gradient(i - width - 1)),
                g.wrapping_sub(gradient(i - width + 1)),
                a,
            ];
            for (hist, value) in counts.iter_mut().zip(values) {
                hist[value as usize] += 1;
            }
            points += 1;
        }
    }
    let [
        left,
        top,
        diagonal,
        gradient,
        grad_left,
        grad_top_left,
        grad_top_right,
        raw,
    ] = counts.each_ref().map(entropy_score);
    let outer = left.max(top).max(gradient).max(raw);
    if grad_left.max(grad_top_left).max(grad_top_right) > outer + 6 * points {
        Some(Predictor::Gradient)
    } else if diagonal > outer + 2 * points {
        Some(Predictor::None)
    } else {
        None
    }
}

pub(super) fn palette_predictor(
    pixels: &[[u8; 4]],
    width: usize,
    height: usize,
    current: u8,
) -> Option<Predictor> {
    if width < 16 || height < 16 {
        return None;
    }
    let mut histogram = [[0u32; 256]; 2];
    for y in (1..height).step_by(17) {
        for x in (1..width).step_by(17) {
            let i = y * width + x;
            let [a, l, t, c] = [i, i - 1, i - width, i - width - 1].map(|k| pixels[k][3]);
            histogram[0][a.wrapping_sub(l) as usize] += 1;
            histogram[1][a.wrapping_sub(super::gradient(l, t, c)) as usize] += 1;
        }
    }
    [Predictor::Horizontal, Predictor::Gradient]
        .into_iter()
        .zip(histogram.iter())
        .filter(|(p, h)| {
            current != (1 | (*p as u8) << 2) && h.iter().filter(|&&n| n != 0).count() <= 10
        })
        .max_by_key(|(_, h)| entropy_score(h))
        .map(|(p, _)| p)
}

pub(super) fn encode(
    samples: &mut [u8],
    width: usize,
    method: u8,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let side = if method == VP8L_ALPHA { 32 } else { 64 };
    encode_tiles(
        samples,
        width,
        method,
        limit,
        side,
        TileSearch::Basic,
        cancelled,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TileSearch {
    Basic,
    Detailed,
    HighOpacity,
    SparseEdges,
}

pub(super) fn encode_tiles(
    samples: &mut [u8],
    width: usize,
    method: u8,
    limit: usize,
    side: usize,
    search: TileSearch,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let modes: &[u8] = match search {
        TileSearch::HighOpacity => &[5, 7, 13],
        TileSearch::SparseEdges => &[1, 2, 3, 4, 12],
        TileSearch::Detailed => &ALL_MODES,
        _ => &MODES,
    };
    let stride = if search != TileSearch::Basic { 2 } else { 4 };
    let columns = width.div_ceil(side);
    let rows = (samples.len() / width).div_ceil(side);
    let mut predictors = Vec::with_capacity(columns * rows);
    let mut counts = [0; GREEN_SYMBOLS];
    for y in (0..samples.len() / width).step_by(side) {
        check_cancelled(cancelled)?;
        for x in (0..width).step_by(side) {
            let mut histogram = [[0u16; 256]; ALL_MODES.len()];
            let y_end = (y + side).min(samples.len() / width);
            let x_end = (x + side).min(width);
            for y in (y.max(1)..y_end).step_by(stride) {
                for x in (x.max(1)..x_end).step_by(stride) {
                    let i = y * width + x;
                    let [l, t, c, r] = neighbors(samples, i, x, width);
                    if search == TileSearch::Detailed {
                        let lt = mean(l, t);
                        let lc = mean(l, c);
                        let tr = mean(t, r);
                        let guesses = [
                            0,
                            l,
                            t,
                            r,
                            c,
                            mean(mean(l, r), t),
                            lc,
                            lt,
                            mean(c, t),
                            tr,
                            mean(lc, tr),
                            if t.abs_diff(c) < l.abs_diff(c) { l } else { t },
                            super::gradient(l, t, c),
                            super::half(l, t, c),
                        ];
                        for (h, guess) in histogram.iter_mut().zip(guesses) {
                            h[samples[i].wrapping_sub(guess) as usize] += 1;
                        }
                    } else {
                        for (h, mode) in histogram.iter_mut().zip(modes.iter().copied()) {
                            h[samples[i].wrapping_sub(predict(mode, l, t, c, r)) as usize] += 1;
                        }
                    }
                }
            }
            let mode = histogram
                .iter()
                .zip(modes.iter().copied())
                .max_by_key(|(h, mode)| (tile_entropy(h), std::cmp::Reverse(*mode)))
                .unwrap()
                .1;
            predictors.push(mode);
            counts[mode as usize] += 1;
        }
    }
    // Keep the left and upper neighbors intact until their residuals are written.
    for y in (0..samples.len() / width).rev() {
        check_cancelled(cancelled)?;
        for x in (0..width).rev() {
            let i = y * width + x;
            let guess = if y == 0 {
                if x == 0 { 0 } else { samples[i - 1] }
            } else if x == 0 {
                samples[i - width]
            } else {
                let [l, t, c, r] = neighbors(samples, i, x, width);
                predict(predictors[(y / side) * columns + x / side], l, t, c, r)
            };
            samples[i] = samples[i].wrapping_sub(guess);
        }
    }
    let mut bits = Bits::header(method);
    bits.write(1, 3);
    bits.write(side.ilog2() - 2, 3);
    bits.write(0, 1);
    let codes = bits.write_green_codes(&counts);
    for value in [0, 0, 0, 0] {
        bits.write_single_tree(value);
    }
    bits.write_literals(&predictors, &codes, cancelled)?;
    bits.write(0, 1);
    let copies = matches(
        samples,
        width,
        match search {
            TileSearch::HighOpacity => 5,
            TileSearch::SparseEdges => 8,
            _ => 4,
        },
        cancelled,
    )?;
    let tiles = match search {
        TileSearch::HighOpacity => groups::clustered(samples, width, &copies, 16, 5, 2, cancelled)?,
        TileSearch::SparseEdges => groups::clustered(samples, width, &copies, 32, 3, 3, cancelled)?,
        TileSearch::Detailed => groups::squared_residuals(samples, width, cancelled)?,
        _ => groups::classify(samples, width, cancelled)?
            .unwrap_or_else(|| groups::Tiles::single(samples.len(), width)),
    };
    let literals = groups::codes_for(&tiles, samples, width, bits.clone(), 0, &[], cancelled)?;
    let copied = groups::codes_for(&tiles, samples, width, bits, 0, &copies, cancelled)?;
    let (codes, copies) = if copied.len < literals.len {
        (copied, copies.as_slice())
    } else {
        (literals, &[][..])
    };
    if codes.len >= limit.next_multiple_of(2) {
        return Ok(None);
    }
    Ok(Some(
        codes.write(&tiles, samples, width, copies, cancelled)?,
    ))
}

fn neighbors(samples: &[u8], i: usize, x: usize, width: usize) -> [u8; 4] {
    [
        samples[i - 1],
        samples[i - width],
        samples[i - width - 1],
        if x + 1 == width {
            samples[i - x]
        } else {
            samples[i - width + 1]
        },
    ]
}

fn predict(mode: u8, l: u8, t: u8, c: u8, r: u8) -> u8 {
    match mode {
        0 => 0,
        1 => l,
        2 => t,
        3 => r,
        4 => c,
        5 => mean(mean(l, r), t),
        6 => mean(l, c),
        8 => mean(c, t),
        9 => mean(t, r),
        10 => mean(mean(l, c), mean(t, r)),
        7 => mean(l, t),
        11 => {
            if t.abs_diff(c) < l.abs_diff(c) {
                l
            } else {
                t
            }
        }
        12 => super::gradient(l, t, c),
        13 => super::half(l, t, c),
        _ => unreachable!(),
    }
}

fn matches(
    samples: &[u8],
    width: usize,
    minimum: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<(u32, u16)>, Error> {
    let mut copies = Vec::new();
    let mut at = 0;
    let mut poll = 0;
    while at + minimum <= samples.len() {
        if at >= poll {
            check_cancelled(cancelled)?;
            poll = at + 4096;
        }
        let window = &samples[at..(at + 4096).min(samples.len())];
        let prefix = window.first_chunk::<4>().unwrap();
        let mut best = (minimum - 1, 0);
        for (index, distance) in [width, 1].into_iter().enumerate() {
            if let Some(from) = at.checked_sub(distance) {
                let past = &samples[from..from + window.len()];
                if prefix == past.first_chunk::<4>().unwrap() && best.0 < window.len() {
                    let n = 4 + common_prefix(&window[4..], &past[4..]);
                    if n > best.0 {
                        best = (n, index);
                    }
                }
            }
        }
        if best.0 >= minimum {
            copies.push((at as u32, (best.0 as u16 - 1) | (best.1 as u16) << 12));
            at += best.0;
        } else {
            at += 1;
        }
    }
    Ok(copies)
}

pub(super) fn average(
    samples: &mut [u8],
    width: usize,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    for y in (1..samples.len() / width).rev() {
        check_cancelled(cancelled)?;
        let start = y * width;
        for x in (1..width).rev() {
            let i = start + x;
            samples[i] = samples[i].wrapping_sub(
                ((u16::from(samples[i - 1]) + u16::from(samples[i - width])) / 2) as u8,
            );
        }
        samples[start] = samples[start].wrapping_sub(samples[start - width]);
    }
    for x in (1..width).rev() {
        samples[x] = samples[x].wrapping_sub(samples[x - 1]);
    }
    let mut bits = Bits::header(VP8L_ALPHA);
    bits.write(1, 3);
    bits.write(3, 3);
    bits.write(0, 1);
    for value in [7, 0, 0, 0, 0] {
        bits.write_single_tree(value);
    }
    bits.write(0, 1);
    let single = groups::Tiles::single(samples.len(), width);
    let flats = groups::codes_for(&single, samples, width, bits.clone(), 0, &[], cancelled)?;
    let tiles = groups::classify(samples, width, cancelled)?;
    if let Some(tiles) = tiles {
        let grouped = groups::codes_for(&tiles, samples, width, bits, 0, &[], cancelled)?;
        if grouped.len < flats.len && grouped.len < limit.next_multiple_of(2) {
            return grouped
                .write(&tiles, samples, width, &[], cancelled)
                .map(Some);
        }
    }
    if flats.len < limit.next_multiple_of(2) {
        return flats
            .write(&single, samples, width, &[], cancelled)
            .map(Some);
    }
    Ok(None)
}

fn mean(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b)) / 2) as u8
}

pub(super) fn detailed_predictor(
    pixels: &[[u8; 4]],
    width: usize,
    height: usize,
    limit: usize,
) -> Option<(Predictor, usize)> {
    let mut counts = [[0u32; 256]; 5];
    let mut partial = 0;
    let mut points = 0;
    for y in (2..height).step_by(11) {
        for x in (2..width - 1).step_by(11) {
            let i = y * width + x;
            let [a, l, t, c] = [i, i - 1, i - width, i - width - 1].map(|i| pixels[i][3]);
            let grad = super::gradient(l, t, c);
            let half = super::half(l, t, c);
            for (hist, b) in counts.iter_mut().zip([l, t, c, grad, half]) {
                hist[a.wrapping_sub(b) as usize] += 1;
            }
            partial += usize::from(a > 0 && a < 255);
            points += 1;
        }
    }
    let [left, top, corner, grad, half] = counts.each_ref().map(entropy_score);
    if limit * 3 >= pixels.len() {
        if corner > left.max(top).max(grad).max(half) + 2 * points as u32 {
            return Some((Predictor::None, 32));
        }
        if partial * 4 >= points * 3 && half + 2 * points as u32 >= grad {
            let unique = counts[3].iter().filter(|&&n| n != 0).count();
            if unique > 20 && grad + 2 * points as u32 >= left.max(top).max(half) {
                return Some((Predictor::Gradient, 32));
            }
            return Some((Predictor::None, 64));
        }
    }
    if partial * 3 >= points * 2 && limit * 4 >= pixels.len() && half > grad + points as u32 {
        return Some((Predictor::None, 64));
    }
    None
}

fn tile_entropy(histogram: &[u16; 256]) -> u32 {
    // A 64×64 predictor tile sampled every second pixel has at most 1,024 samples.
    const COSTS: [u32; 1025] = {
        let mut costs = [0; 1025];
        let mut count = 2;
        while count < costs.len() {
            costs[count] = super::entropy_cost(count as u32);
            count += 1;
        }
        costs
    };
    histogram.iter().map(|&n| COSTS[usize::from(n)]).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layered_prediction_round_trips_right_edge() {
        let (width, height) = (133, 81);
        let rgba: Vec<_> = (0..height)
            .flat_map(|y| {
                (0..width)
                    .flat_map(move |x| [30, 70, 110, ((x * 3 + y * 7 + (x + y) % 11) & 255) as u8])
            })
            .collect();
        let mut residual = vec![0; width * height];
        super::super::filter(
            Predictor::Gradient,
            rgba.as_chunks::<4>().0,
            width,
            &mut residual,
            &|| false,
        )
        .unwrap();
        let alpha = encode(&mut residual, width, 13, usize::MAX / 2, &|| false)
            .unwrap()
            .unwrap();
        let mut options = crate::EncodeOptions::with_workers(80, 1);
        options.has_transparency = Some(false);
        let mut image =
            crate::encoder::encode(&rgba, width as u32, height as u32, options, &|| false).unwrap();
        image.alpha = Some(alpha);
        let decoded = image::load_from_memory(&image.into_bytes())
            .unwrap()
            .into_rgba8();
        assert!(
            decoded
                .as_raw()
                .iter()
                .skip(3)
                .step_by(4)
                .eq(rgba.iter().skip(3).step_by(4))
        );
    }
}
