//! VP8L predictor-image coding over the ALPH residual plane.

use super::repeated::{Bits, GREEN_SYMBOLS, common_prefix};
use super::{Predictor, VP8L_ALPHA, entropy_score, groups};
use crate::{Error, encoder::check_cancelled};

const MODES: [u8; 8] = [0, 1, 2, 3, 4, 11, 12, 13];
const ALL_MODES: [u8; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

pub(super) fn outer(px: &[[u8; 4]], w: usize, h: usize) -> Option<Predictor> {
    let mut hists = [[0u32; 256]; 8];
    let mut pts = 0;
    let grad = |i: usize| {
        px[i][3].wrapping_sub(super::gradient(
            px[i - 1][3],
            px[i - w][3],
            px[i - w - 1][3],
        ))
    };
    for y in (2..h).step_by(11) {
        for x in (2..w - 1).step_by(11) {
            let i = y * w + x;
            let a = px[i][3];
            let l = px[i - 1][3];
            let t = px[i - w][3];
            let c = px[i - w - 1][3];
            let g = grad(i);
            let vals = [
                a.wrapping_sub(l),
                a.wrapping_sub(t),
                a.wrapping_sub(c),
                g,
                g.wrapping_sub(grad(i - 1)),
                g.wrapping_sub(grad(i - w - 1)),
                g.wrapping_sub(grad(i - w + 1)),
                a,
            ];
            for (hist, v) in hists.iter_mut().zip(vals) {
                hist[v as usize] += 1;
            }
            pts += 1;
        }
    }
    let [left, top, diag, grad_s, g_left, g_tl, g_tr, raw] = hists.each_ref().map(entropy_score);
    let best = left.max(top).max(grad_s).max(raw);
    if g_left.max(g_tl).max(g_tr) > best + 6 * pts {
        Some(Predictor::Gradient)
    } else if diag > best + 2 * pts {
        Some(Predictor::None)
    } else {
        None
    }
}

pub(super) fn palette_predictor(px: &[[u8; 4]], w: usize, h: usize, cur: u8) -> Option<Predictor> {
    if w < 16 || h < 16 {
        return None;
    }
    let mut hist = [[0u32; 256]; 2];
    for y in (1..h).step_by(17) {
        for x in (1..w).step_by(17) {
            let i = y * w + x;
            let [a, l, t, c] = [i, i - 1, i - w, i - w - 1].map(|k| px[k][3]);
            hist[0][a.wrapping_sub(l) as usize] += 1;
            hist[1][a.wrapping_sub(super::gradient(l, t, c)) as usize] += 1;
        }
    }
    [Predictor::Horizontal, Predictor::Gradient]
        .into_iter()
        .zip(hist.iter())
        .filter(|(p, h)| {
            cur != (1 | (*p as u8) << 2) && h.iter().filter(|&&n| n != 0).count() <= 10
        })
        .max_by_key(|(_, h)| entropy_score(h))
        .map(|(p, _)| p)
}

pub(super) fn encode(
    src: &mut [u8],
    w: usize,
    method: u8,
    limit: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let side = if method == VP8L_ALPHA { 32 } else { 64 };
    encode_tiles(src, w, method, limit, side, TileSearch::Basic, cancel)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TileSearch {
    Basic,
    Detailed,
    HighOpacity,
    SparseEdges,
}

pub(super) fn encode_tiles(
    src: &mut [u8],
    w: usize,
    method: u8,
    limit: usize,
    side: usize,
    search: TileSearch,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let modes: &[u8] = match search {
        TileSearch::HighOpacity => &[5, 7, 13],
        TileSearch::SparseEdges => &[1, 2, 3, 4, 12],
        TileSearch::Detailed => &ALL_MODES,
        TileSearch::Basic => &MODES,
    };
    let stride = if search != TileSearch::Basic { 2 } else { 4 };
    let h = src.len() / w;
    let cols = w.div_ceil(side);
    let rows = h.div_ceil(side);
    let mut preds = Vec::with_capacity(cols * rows);
    let mut hist = [0; GREEN_SYMBOLS];
    for y0 in (0..h).step_by(side) {
        check_cancelled(cancel)?;
        for x0 in (0..w).step_by(side) {
            let mut tile_hist = [[0u16; 256]; ALL_MODES.len()];
            let y1 = (y0 + side).min(h);
            let x1 = (x0 + side).min(w);
            for y in (y0.max(1)..y1).step_by(stride) {
                for x in (x0.max(1)..x1).step_by(stride) {
                    let i = y * w + x;
                    let [l, t, c, r] = neighbors(src, i, x, w);
                    if search == TileSearch::Detailed {
                        let lt = mean(l, t);
                        let lc = mean(l, c);
                        let tr = mean(t, r);
                        let candidates = [
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
                        for (h, pred) in tile_hist.iter_mut().zip(candidates) {
                            h[src[i].wrapping_sub(pred) as usize] += 1;
                        }
                    } else {
                        for (h, &mode) in tile_hist.iter_mut().zip(modes) {
                            h[src[i].wrapping_sub(predict(mode, l, t, c, r)) as usize] += 1;
                        }
                    }
                }
            }
            let mode = tile_hist
                .iter()
                .zip(modes.iter().copied())
                .max_by_key(|(h, mode)| (tile_entropy(h), std::cmp::Reverse(*mode)))
                .unwrap()
                .1;
            preds.push(mode);
            hist[mode as usize] += 1;
        }
    }
    // Reverse raster keeps left and upper neighbors intact until their residuals are written.
    for y in (0..h).rev() {
        check_cancelled(cancel)?;
        for x in (0..w).rev() {
            let i = y * w + x;
            let pred = if y == 0 {
                if x == 0 { 0 } else { src[i - 1] }
            } else if x == 0 {
                src[i - w]
            } else {
                let [l, t, c, r] = neighbors(src, i, x, w);
                predict(preds[(y / side) * cols + x / side], l, t, c, r)
            };
            src[i] = src[i].wrapping_sub(pred);
        }
    }
    let mut bits = Bits::header(method);
    bits.write(1, 3);
    bits.write(side.ilog2() - 2, 3);
    bits.write(0, 1);
    let pred_codes = bits.write_green_codes(&hist);
    for v in [0, 0, 0, 0] {
        bits.write_single_tree(v);
    }
    bits.write_literals(&preds, &pred_codes, cancel)?;
    bits.write(0, 1);
    let copies = matches(
        src,
        w,
        match search {
            TileSearch::HighOpacity => 5,
            TileSearch::SparseEdges => 8,
            _ => 4,
        },
        cancel,
    )?;
    let tiles = match search {
        TileSearch::HighOpacity => groups::clustered(src, w, &copies, 16, 5, 2, cancel)?,
        TileSearch::SparseEdges => groups::clustered(src, w, &copies, 32, 3, 3, cancel)?,
        TileSearch::Detailed => groups::squared_residuals(src, w, cancel)?,
        TileSearch::Basic => {
            groups::classify(src, w, cancel)?.unwrap_or_else(|| groups::Tiles::single(src.len(), w))
        }
    };
    let literals = groups::codes_for(&tiles, src, w, bits.clone(), 0, &[], cancel)?;
    let copied = groups::codes_for(&tiles, src, w, bits, 0, &copies, cancel)?;
    let (codes, copies) = if copied.len < literals.len {
        (copied, copies.as_slice())
    } else {
        (literals, &[][..])
    };
    if codes.len >= limit.next_multiple_of(2) {
        return Ok(None);
    }
    Ok(Some(codes.write(&tiles, src, w, copies, cancel)?))
}

fn neighbors(src: &[u8], i: usize, x: usize, w: usize) -> [u8; 4] {
    [
        src[i - 1],
        src[i - w],
        src[i - w - 1],
        if x + 1 == w {
            src[i - x]
        } else {
            src[i - w + 1]
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
        7 => mean(l, t),
        8 => mean(c, t),
        9 => mean(t, r),
        10 => mean(mean(l, c), mean(t, r)),
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
    src: &[u8],
    w: usize,
    min_len: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<(u32, u16)>, Error> {
    let mut copies = Vec::new();
    let mut at = 0;
    let mut poll = 0;
    while at + min_len <= src.len() {
        if at >= poll {
            check_cancelled(cancel)?;
            poll = at + 4096;
        }
        let win = &src[at..(at + 4096).min(src.len())];
        let pfx = win.first_chunk::<4>().unwrap();
        let mut best = (min_len - 1, 0);
        for (i, dist) in [w, 1].into_iter().enumerate() {
            if let Some(from) = at.checked_sub(dist) {
                let past = &src[from..from + win.len()];
                if pfx == past.first_chunk::<4>().unwrap() && best.0 < win.len() {
                    let n = 4 + common_prefix(&win[4..], &past[4..]);
                    if n > best.0 {
                        best = (n, i);
                    }
                }
            }
        }
        if best.0 >= min_len {
            copies.push((at as u32, (best.0 as u16 - 1) | (best.1 as u16) << 12));
            at += best.0;
        } else {
            at += 1;
        }
    }
    Ok(copies)
}

pub(super) fn average(
    src: &mut [u8],
    w: usize,
    limit: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    for y in (1..src.len() / w).rev() {
        check_cancelled(cancel)?;
        let start = y * w;
        for x in (1..w).rev() {
            let i = start + x;
            src[i] =
                src[i].wrapping_sub(((u16::from(src[i - 1]) + u16::from(src[i - w])) / 2) as u8);
        }
        src[start] = src[start].wrapping_sub(src[start - w]);
    }
    for x in (1..w).rev() {
        src[x] = src[x].wrapping_sub(src[x - 1]);
    }
    let mut bits = Bits::header(VP8L_ALPHA);
    bits.write(1, 3);
    bits.write(3, 3);
    bits.write(0, 1);
    for v in [7, 0, 0, 0, 0] {
        bits.write_single_tree(v);
    }
    bits.write(0, 1);
    let single = groups::Tiles::single(src.len(), w);
    let flats = groups::codes_for(&single, src, w, bits.clone(), 0, &[], cancel)?;
    let tiles = groups::classify(src, w, cancel)?;
    if let Some(tiles) = tiles {
        let grouped = groups::codes_for(&tiles, src, w, bits, 0, &[], cancel)?;
        if grouped.len < flats.len && grouped.len < limit.next_multiple_of(2) {
            return grouped.write(&tiles, src, w, &[], cancel).map(Some);
        }
    }
    if flats.len < limit.next_multiple_of(2) {
        return flats.write(&single, src, w, &[], cancel).map(Some);
    }
    Ok(None)
}

fn mean(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b)) / 2) as u8
}

pub(super) fn detailed_predictor(
    px: &[[u8; 4]],
    w: usize,
    h: usize,
    limit: usize,
) -> Option<(Predictor, usize)> {
    let mut hists = [[0u32; 256]; 5];
    let mut partial = 0;
    let mut pts = 0;
    for y in (2..h).step_by(11) {
        for x in (2..w - 1).step_by(11) {
            let i = y * w + x;
            let [a, l, t, c] = [i, i - 1, i - w, i - w - 1].map(|k| px[k][3]);
            let grad = super::gradient(l, t, c);
            let half = super::half(l, t, c);
            for (hist, b) in hists.iter_mut().zip([l, t, c, grad, half]) {
                hist[a.wrapping_sub(b) as usize] += 1;
            }
            partial += usize::from(a > 0 && a < 255);
            pts += 1;
        }
    }
    let [left, top, corner, grad, half] = hists.each_ref().map(entropy_score);
    if limit * 3 >= px.len() {
        if corner > left.max(top).max(grad).max(half) + 2 * pts as u32 {
            return Some((Predictor::None, 32));
        }
        if partial * 4 >= pts * 3 && half + 2 * pts as u32 >= grad {
            let unique = hists[3].iter().filter(|&&n| n != 0).count();
            if unique > 20 && grad + 2 * pts as u32 >= left.max(top).max(half) {
                return Some((Predictor::Gradient, 32));
            }
            return Some((Predictor::None, 64));
        }
    }
    if partial * 3 >= pts * 2 && limit * 4 >= px.len() && half > grad + pts as u32 {
        return Some((Predictor::None, 64));
    }
    None
}

fn tile_entropy(hist: &[u16; 256]) -> u32 {
    // 64x64 tile sampled every 2nd pixel has <= 1024 samples.
    const COSTS: [u32; 1025] = {
        let mut costs = [0; 1025];
        let mut count = 2;
        while count < costs.len() {
            costs[count] = super::entropy_cost(count as u32);
            count += 1;
        }
        costs
    };
    hist.iter().map(|&n| COSTS[usize::from(n)]).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_pred_right_edge() {
        let (w, h) = (133, 81);
        let rgba: Vec<_> = (0..h)
            .flat_map(|y| {
                (0..w)
                    .flat_map(move |x| [30, 70, 110, ((x * 3 + y * 7 + (x + y) % 11) & 255) as u8])
            })
            .collect();
        let mut res = vec![0; w * h];
        super::super::filter(
            Predictor::Gradient,
            rgba.as_chunks::<4>().0,
            w,
            &mut res,
            &|| false,
        )
        .unwrap();
        let alpha = encode(&mut res, w, 13, usize::MAX / 2, &|| false)
            .unwrap()
            .unwrap();
        let mut opts = crate::EncodeOptions::with_workers(80, 1);
        opts.has_transparency = Some(false);
        let mut webp = crate::encoder::encode(&rgba, w as u32, h as u32, opts, &|| false).unwrap();
        webp.alpha = Some(alpha);
        let decoded = image::load_from_memory(&webp.into_bytes())
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
