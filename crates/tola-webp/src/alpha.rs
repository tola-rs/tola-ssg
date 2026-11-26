//! Lossless VP8L alpha payloads for extended WebP images.

use crate::Error;
use crate::encoder::check_cancelled;

mod groups;
mod palette;
mod repeated;
mod rle;
mod spatial;

const RAW_ALPHA: u8 = 0;
const VP8L_ALPHA: u8 = 1;
const PIXELS_PER_CHECK: usize = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Predictor {
    None = 0,
    Horizontal = 1,
    Vertical = 2,
    Gradient = 3,
    Half,
}

pub(crate) fn encode(
    rgba: &[u8],
    w: usize,
    h: usize,
    has_transparency: Option<bool>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    check_cancelled(cancel)?;
    if has_transparency == Some(false) {
        return Ok(None);
    }

    let raw_len = w * h + 1;
    let first = rgba[3];
    if rgba.last() == Some(&first)
        && (first != u8::MAX || has_transparency.is_none())
        && uniform_alpha(rgba, first, cancel)?
    {
        if first == u8::MAX {
            return Ok(None);
        }
        // 1x1 VP8L frame holds one Huffman symbol; ALPH supplies actual dimensions.
        let alpha = rle::encode(&[first], Predictor::None.stream(), raw_len, cancel)?
            .unwrap_or_else(|| {
                let mut raw = vec![first; raw_len];
                raw[0] = RAW_ALPHA;
                raw
            });
        check_cancelled(cancel)?;
        return Ok(Some(alpha));
    }

    let px = rgba.as_chunks::<4>().0;
    let mut alpha = vec![0; raw_len];
    let ([alt, rep], try_half) = alternative_predictors(px, w, h);
    let unfiltered =
        (px.len() <= 512 * 1024 && alt != Some(Predictor::None)).then_some(Predictor::None);
    let mut best = encode_predictors(
        px,
        w,
        &mut alpha[1..],
        raw_len,
        [Some(Predictor::Vertical), unfiltered, alt, rep]
            .into_iter()
            .flatten(),
        cancel,
    )?;
    if try_half
        && let Some(half) =
            encode_predictors(px, w, &mut alpha[1..], raw_len, [Predictor::Half], cancel)?
        && best
            .as_ref()
            .is_none_or(|prev| half.len().next_multiple_of(2) < prev.len().next_multiple_of(2))
    {
        best = Some(half);
    }
    if let Some(bytes) = best.as_ref()
        && bytes.len() >= 4096
        && bytes.len() * 12 >= px.len()
        && (64 * 1024..=1024 * 1024).contains(&px.len())
        && let Some(pred) = spatial::palette_predictor(px, w, h, bytes[0])
    {
        filter(pred, px, w, &mut alpha[1..], cancel)?;
        let method = VP8L_ALPHA | (pred as u8) << 2;
        if let Some(bytes) =
            palette::encode(&mut alpha[1..], w, h, method, bytes.len(), true, cancel)?
        {
            best = Some(bytes);
        }
    }
    if (64 * 1024..=512 * 1024).contains(&px.len())
        && w >= 16
        && h >= 16
        && best.as_ref().map_or(raw_len, Vec::len) * 3 >= px.len()
    {
        filter(Predictor::None, px, w, &mut alpha[1..], cancel)?;
        let limit = best.as_ref().map_or(raw_len, Vec::len);
        if let Some(bytes) = spatial::average(&mut alpha[1..], w, limit, cancel)? {
            best = Some(bytes);
        }
    }
    let limit = best.as_ref().map_or(raw_len, Vec::len);
    if (64 * 1024..=1024 * 1024).contains(&px.len())
        && w >= 16
        && h >= 16
        && limit * 6 >= px.len()
        && let Some(pred) = spatial::outer(px, w, h)
    {
        filter(pred, px, w, &mut alpha[1..], cancel)?;
        let method = VP8L_ALPHA | (pred as u8) << 2;
        if let Some(bytes) = spatial::encode(&mut alpha[1..], w, method, limit, cancel)? {
            best = Some(bytes);
        }
    }
    let limit = best.as_ref().map_or(raw_len, Vec::len);
    if w >= 16
        && h >= 16
        && (64 * 1024..=1024 * 1024).contains(&px.len())
        && let Some((pred, side)) = spatial::detailed_predictor(px, w, h, limit)
    {
        filter(pred, px, w, &mut alpha[1..], cancel)?;
        if let Some(cand) = spatial::encode_tiles(
            &mut alpha[1..],
            w,
            VP8L_ALPHA | (pred as u8) << 2,
            limit,
            side,
            spatial::TileSearch::Detailed,
            cancel,
        )? {
            best = Some(cand);
        }
    }
    if w >= 16 && h >= 16 && (64 * 1024..=1024 * 1024).contains(&px.len()) {
        let mut below = 0;
        let mut partial = 0;
        let mut count = 0;
        for p in px.iter().step_by((px.len() / 2048).max(1)) {
            below += usize::from(p[3] < 128);
            partial += usize::from(p[3] > 0 && p[3] < 255);
            count += 1;
        }
        let limit = best.as_ref().map_or(raw_len, Vec::len);
        let cfg = if below == 0 && partial * 2 >= count && limit * 4 >= px.len() {
            Some((64, spatial::TileSearch::HighOpacity))
        } else if partial * 32 >= count && partial * 12 <= count && limit * 40 >= px.len() {
            Some((32, spatial::TileSearch::SparseEdges))
        } else {
            None
        };
        if let Some((side, kind)) = cfg {
            filter(Predictor::None, px, w, &mut alpha[1..], cancel)?;
            if let Some(cand) =
                spatial::encode_tiles(&mut alpha[1..], w, VP8L_ALPHA, limit, side, kind, cancel)?
            {
                best = Some(cand);
            }
        }
    }
    if best.is_some() {
        return Ok(best);
    }

    for (raw, chunk) in alpha[1..]
        .chunks_mut(PIXELS_PER_CHECK)
        .zip(px.chunks(PIXELS_PER_CHECK))
    {
        check_cancelled(cancel)?;
        for (dst, p) in raw.iter_mut().zip(chunk) {
            *dst = p[3];
        }
    }
    Ok(Some(alpha))
}

fn encode_predictors(
    px: &[[u8; 4]],
    w: usize,
    res: &mut [u8],
    limit: usize,
    preds: impl IntoIterator<Item = Predictor>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let mut best = None::<(Predictor, repeated::AlphaBlock)>;
    let mut last = Predictor::Vertical;
    for pred in preds {
        last = pred;
        filter(pred, px, w, res, cancel)?;
        let stream = pred.stream();
        let cap = best.as_ref().map_or(limit, |(_, p)| p.len());
        if let Some(compressed) = repeated::encode(res, w, stream, cap, cancel)? {
            best = Some((pred, compressed));
        }
        let cap = best.as_ref().map_or(limit, |(_, p)| p.len());
        if let Some(compressed) = rle::encode(res, stream, cap, cancel)? {
            best = Some((pred, repeated::AlphaBlock::Bytes(compressed)));
        }
    }
    if let Some((pred, best)) = best {
        let try_palette = pred != Predictor::Half
            && best.len() >= 4096
            && (64 * 1024..=1024 * 1024).contains(&res.len());
        if pred != last && (try_palette || matches!(best, repeated::AlphaBlock::Tiles(_))) {
            filter(pred, px, w, res, cancel)?;
        }
        let best = best.finish(res, w, cancel)?;
        if try_palette
            && let Some(cand) =
                palette::encode(res, w, res.len() / w, best[0], best.len(), false, cancel)?
        {
            return Ok(Some(cand));
        }
        return Ok(Some(best));
    }
    Ok(None)
}

pub(crate) fn has_transparency(
    rgba: &[u8],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<bool, Error> {
    Ok(!uniform_alpha(rgba, u8::MAX, cancel)?)
}

fn alternative_predictors(px: &[[u8; 4]], w: usize, h: usize) -> ([Option<Predictor>; 2], bool) {
    if w < 2 || h < 2 {
        return ([None; 2], false);
    }
    const SAMPLES: u32 = 2048;
    let try_half = (65_536..=512 * 1024).contains(&px.len());
    let mut hists = [[0u32; 256]; 4];
    let mut half_hist = [0u32; 256];
    let mut repeated = [0u32; 4];
    let mut seed = 0x71a5_b963u32;
    for _ in 0..SAMPLES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = 1 + (seed >> 12) as usize % (w - 1);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let y = 1 + (seed >> 12) as usize % (h - 1);
        let at = y * w + x;
        let [val, left, top, corner] = [at, at - 1, at - w, at - w - 1].map(|i| px[i][3]);
        for (hist, pred) in hists
            .iter_mut()
            .zip([left, top, gradient(left, top, corner), 0])
        {
            hist[usize::from(val.wrapping_sub(pred))] += 1;
        }
        if try_half {
            half_hist[usize::from(val.wrapping_sub(half(left, top, corner)))] += 1;
        }
        if y > 1 {
            let above = at - w;
            let [above_left, prev, prev_left] =
                [above - 1, above - w, above - w - 1].map(|i| px[i][3]);
            let cur = [left, top, gradient(left, top, corner), 0];
            let prev = [above_left, prev, gradient(above_left, prev, prev_left), 0];
            for (i, count) in repeated.iter_mut().enumerate() {
                *count += u32::from(val.wrapping_sub(cur[i]) == top.wrapping_sub(prev[i]));
            }
        }
    }
    let scores = hists.map(|hist| hist.iter().map(|n| n * n).sum::<u32>());
    let alt = [Predictor::Horizontal, Predictor::Gradient, Predictor::None]
        .into_iter()
        .filter(|&p| scores[p.score_index()] > scores[1])
        .max_by_key(|&p| scores[p.score_index()]);
    let try_half = if try_half {
        let [horiz, vert, grad, raw] = hists.each_ref().map(entropy_score);
        let half_score = entropy_score(&half_hist);
        let best_linear = horiz.max(vert).max(raw);
        half_score > best_linear + SAMPLES / 2 && half_score + SAMPLES >= grad
    } else {
        false
    };
    let included = alt.map_or(repeated[1], |p| repeated[1].max(repeated[p.score_index()]));
    let row_repeats = [Predictor::Horizontal, Predictor::Gradient]
        .into_iter()
        .filter(|&p| repeated[p.score_index()] > included + SAMPLES / 12)
        .max_by_key(|&p| repeated[p.score_index()]);
    ([alt, row_repeats], try_half)
}

fn entropy_score(counts: &[u32; 256]) -> u32 {
    counts.iter().copied().map(entropy_cost).sum()
}

const fn entropy_cost(n: u32) -> u32 {
    const LOG_FRACTION: [u32; 16] = [0, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15];
    if n < 2 {
        return 0;
    }
    let log = n.ilog2();
    let norm = (n << (31 - log)) >> 27;
    n * (16 * log + LOG_FRACTION[(norm & 15) as usize])
}

#[derive(Clone, Copy)]
enum Stream {
    Filtered(u8),
    HalfGradient,
}

impl Predictor {
    fn stream(self) -> Stream {
        match self {
            Self::Half => Stream::HalfGradient,
            pred => Stream::Filtered(VP8L_ALPHA | (pred as u8) << 2),
        }
    }

    fn score_index(self) -> usize {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
            Self::Gradient => 2,
            Self::None => 3,
            Self::Half => 4,
        }
    }
}

fn filter(
    pred: Predictor,
    px: &[[u8; 4]],
    w: usize,
    dst: &mut [u8],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    let mut above: &[[u8; 4]] = &[];
    for (y, (row, out)) in px.chunks_exact(w).zip(dst.chunks_exact_mut(w)).enumerate() {
        check_cancelled(cancel)?;
        match (pred, y == 0) {
            (Predictor::None, _) => {
                for (d, p) in out.iter_mut().zip(row) {
                    *d = p[3];
                }
            }
            (Predictor::Vertical, false) => {
                for ((d, p), t) in out.iter_mut().zip(row).zip(above) {
                    *d = p[3].wrapping_sub(t[3]);
                }
            }
            (Predictor::Gradient | Predictor::Half, false) => {
                out[0] = row[0][3].wrapping_sub(above[0][3]);
                for ((d, pair), top) in out[1..]
                    .iter_mut()
                    .zip(row.windows(2))
                    .zip(above.windows(2))
                {
                    let est = if pred == Predictor::Half {
                        half(pair[0][3], top[1][3], top[0][3])
                    } else {
                        gradient(pair[0][3], top[1][3], top[0][3])
                    };
                    *d = pair[1][3].wrapping_sub(est);
                }
            }
            _ => {
                out[0] = row[0][3].wrapping_sub(above.first().map_or(0, |p| p[3]));
                for (d, pair) in out[1..].iter_mut().zip(row.windows(2)) {
                    *d = pair[1][3].wrapping_sub(pair[0][3]);
                }
            }
        }
        above = row;
    }
    Ok(())
}

fn gradient(left: u8, top: u8, corner: u8) -> u8 {
    (i16::from(left) + i16::from(top) - i16::from(corner)).clamp(0, 255) as u8
}

fn half(left: u8, top: u8, corner: u8) -> u8 {
    let avg = (u16::from(left) + u16::from(top)) as i16 / 2;
    (avg + (avg - i16::from(corner)) / 2).clamp(0, 255) as u8
}

fn uniform_alpha(rgba: &[u8], val: u8, cancel: &(dyn Fn() -> bool + Sync)) -> Result<bool, Error> {
    const MASK: u64 = u64::from_ne_bytes([0, 0, 0, 255, 0, 0, 0, 255]);
    let alpha = u64::from_ne_bytes([0, 0, 0, val, 0, 0, 0, val]);
    for chunk in rgba.chunks(4 * PIXELS_PER_CHECK) {
        check_cancelled(cancel)?;
        let (pairs, tail) = chunk.as_chunks::<8>();
        if pairs
            .iter()
            .any(|pair| u64::from_ne_bytes(*pair) & MASK != alpha)
            || tail.get(3).is_some_and(|&last| last != val)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{VP8L_ALPHA, rle};

    #[test]
    fn rle_stream_limit() {
        let mut src: Vec<_> = (0..=u8::MAX).cycle().take(256 * 16).collect();
        assert!(
            rle::encode(
                &src,
                super::Predictor::None.stream(),
                src.len() + 1,
                &|| false
            )
            .unwrap()
            .is_none()
        );

        let mid = (src.len() - 256) / 2;
        src[..mid].fill(0);
        src[mid + 256..].fill(0);
        assert!(
            rle::encode(
                &src,
                super::Predictor::None.stream(),
                src.len() + 1,
                &|| false
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn alpha_roundtrip() {
        for (w, h) in [(1, 1), (1, 7), (8, 3), (17, 19), (32, 33)] {
            for pat in 0..4 {
                let rgba: Vec<u8> = (0..w * h)
                    .flat_map(|i| {
                        let a = match pat {
                            0 => 0,
                            1 => 170,
                            2 => (i % 17) as u8,
                            _ => (i as u8).wrapping_mul(37),
                        };
                        [37, 123, 211, a]
                    })
                    .collect();
                let webp = crate::encode_rgba(&rgba, w, h, 75).unwrap();
                let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
                assert_eq!(decoded.dimensions(), (w, h));
                for (act, exp) in decoded.pixels().zip(rgba.as_chunks::<4>().0) {
                    assert_eq!(act[3], exp[3], "{w}x{h}, pattern {pat}");
                }
            }
        }
    }

    #[test]
    fn repeated_rows_roundtrip() {
        let (w, h) = (129, 97);
        let rgba: Vec<_> = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [37, 123, 211, ((x + y) / 4) as u8]))
            .collect();
        let alpha = super::encode(&rgba, w, h, Some(true), &|| false)
            .unwrap()
            .unwrap();
        assert!(alpha.len() < w * h / 20);
        let webp = crate::encode_rgba(&rgba, w as u32, h as u32, 80).unwrap();
        let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
        assert!(
            decoded
                .as_raw()
                .iter()
                .skip(3)
                .step_by(4)
                .eq(rgba.iter().skip(3).step_by(4))
        );
    }

    #[test]
    fn half_gradient_edges() {
        let (w, h) = (35, 37);
        let px: Vec<_> = (0..h)
            .flat_map(|y| (0..w).map(move |x| [12, 93, 160, (x * 11 + y * 7) as u8]))
            .collect();
        let mut res = vec![0; px.len()];
        let pred = super::Predictor::Half;
        super::filter(pred, &px, w, &mut res, &|| false).unwrap();
        let alpha = rle::encode(&res, pred.stream(), res.len(), &|| false)
            .unwrap()
            .unwrap();
        let mut opts = crate::EncodeOptions::with_workers(80, 1);
        opts.has_transparency = Some(false);
        let mut frame =
            crate::encoder::encode(px.as_flattened(), w as u32, h as u32, opts, &|| false).unwrap();
        frame.alpha = Some(alpha);
        let decoded = image::load_from_memory(&frame.into_bytes())
            .unwrap()
            .into_rgba8();
        assert!(decoded.pixels().map(|p| p[3]).eq(px.iter().map(|p| p[3])));
    }

    #[test]
    fn palette_alpha_roundtrip() {
        let (w, h) = (513usize, 257usize);
        let rgba: Vec<_> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    let band = ((x + 3 * y) / 17) % 8;
                    [12, 93, 160, (band * 31) as u8]
                })
            })
            .collect();
        let mut src: Vec<_> = rgba.iter().copied().skip(3).step_by(4).collect();
        let alpha = super::palette::encode(&mut src, w, h, VP8L_ALPHA, w * h + 1, false, &|| false)
            .unwrap()
            .unwrap();
        let mut opts = crate::EncodeOptions::with_workers(80, 1);
        opts.has_transparency = Some(false);
        let mut webp = crate::encoder::encode(&rgba, w as u32, h as u32, opts, &|| false).unwrap();
        assert!(alpha.len() < w * h / 8);
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
