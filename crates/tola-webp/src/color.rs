//! BT.601 limited-range RGBA-to-YUV420 conversion with macroblock padding.

use std::ops::Range;

use crate::macroblock::{CHROMA_SIDE, MACROBLOCK_SIDE};
use fearless_simd::{Simd, i16x8, prelude::*, u8x16, u16x8};

pub(crate) mod transparent;

const RGBA_CHANNELS: usize = 4;
pub(crate) const YUV_BYTES_PER_MACROBLOCK: usize =
    MACROBLOCK_SIDE * MACROBLOCK_SIDE + 2 * CHROMA_SIDE * CHROMA_SIDE;

#[derive(Clone, Copy)]
pub(crate) struct RgbaFrame<'a> {
    rgba: &'a [u8],
    w: usize,
    h: usize,
    weighted: bool,
}

impl<'a> RgbaFrame<'a> {
    pub(crate) fn new(rgba: &'a [u8], w: usize, h: usize) -> Self {
        Self {
            rgba,
            w,
            h,
            weighted: false,
        }
    }

    pub(crate) fn with_weighted_chroma(mut self, enabled: bool) -> Self {
        self.weighted = enabled;
        self
    }

    pub(crate) fn columns(self) -> usize {
        self.w.div_ceil(MACROBLOCK_SIDE)
    }

    pub(crate) fn rows(self) -> usize {
        self.h.div_ceil(MACROBLOCK_SIDE)
    }

    fn row(self, y: usize, (start, end): (usize, usize)) -> &'a [u8] {
        let off = y * self.w;
        &self.rgba[(off + start) * RGBA_CHANNELS..(off + end) * RGBA_CHANNELS]
    }

    fn px_span(self, cols: Range<usize>) -> (usize, usize) {
        (
            cols.start * MACROBLOCK_SIDE,
            (cols.end * MACROBLOCK_SIDE).min(self.w),
        )
    }

    pub(crate) fn luma_block<S: Simd>(
        self,
        simd: S,
        mb_x: usize,
        mb_y: usize,
    ) -> [u8; MACROBLOCK_SIDE * MACROBLOCK_SIDE] {
        let y0 = mb_y * MACROBLOCK_SIDE;
        let rows = (self.h - y0).min(MACROBLOCK_SIDE);
        let span = self.px_span(mb_x..mb_x + 1);
        let mut blk = [0; MACROBLOCK_SIDE * MACROBLOCK_SIDE];
        for (line, dst) in blk
            .as_chunks_mut::<MACROBLOCK_SIDE>()
            .0
            .iter_mut()
            .take(rows)
            .enumerate()
        {
            luma_row(simd, self.row(y0 + line, span), dst);
        }
        pad_rows(&mut blk, MACROBLOCK_SIDE, rows);
        blk
    }
}

pub(crate) struct YuvRow {
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
    cols: Range<usize>,
}

impl YuvRow {
    pub(crate) fn new(cols: Range<usize>) -> Self {
        let w = cols.len();
        Self {
            y: vec![0; w * MACROBLOCK_SIDE * MACROBLOCK_SIDE],
            u: vec![0; w * CHROMA_SIDE * CHROMA_SIDE],
            v: vec![0; w * CHROMA_SIDE * CHROMA_SIDE],
            cols,
        }
    }

    pub(crate) fn y_stride(&self) -> usize {
        self.y.len() / MACROBLOCK_SIDE
    }

    pub(crate) fn chroma_stride(&self) -> usize {
        self.u.len() / CHROMA_SIDE
    }

    pub(crate) fn local_column(&self, mb_x: usize) -> usize {
        mb_x - self.cols.start
    }

    pub(crate) fn load<S: Simd>(&mut self, simd: S, frame: RgbaFrame<'_>, mb_y: usize) {
        let y0 = mb_y * MACROBLOCK_SIDE;
        let rows = (frame.h - y0).min(MACROBLOCK_SIDE);
        let span = frame.px_span(self.cols.clone());
        let y_stride = self.y_stride();
        let uv_stride = self.chroma_stride();
        let mut uv_rows = rows.div_ceil(2);

        for cell in 0..uv_rows {
            let row = cell * 2;
            let top = frame.row(y0 + row, span);
            let bot = frame.row(y0 + (row + 1).min(rows - 1), span);
            let (top_y, bot_y) = self.y[row * y_stride..][..2 * y_stride].split_at_mut(y_stride);
            row_pair(
                simd,
                frame.weighted,
                top,
                bot,
                top_y,
                bot_y,
                [
                    &mut self.u[cell * uv_stride..][..uv_stride],
                    &mut self.v[cell * uv_stride..][..uv_stride],
                ],
            );
        }
        pad_rows(&mut self.y, y_stride, rows);

        // Even row counts mean the first padding row repeats the last source row.
        if uv_rows < CHROMA_SIDE && rows.is_multiple_of(2) {
            let last = frame.row(y0 + rows - 1, span);
            let (top_y, bot_y) =
                self.y[(2 * uv_rows) * y_stride..][..2 * y_stride].split_at_mut(y_stride);
            row_pair(
                simd,
                frame.weighted,
                last,
                last,
                top_y,
                bot_y,
                [
                    &mut self.u[uv_rows * uv_stride..][..uv_stride],
                    &mut self.v[uv_rows * uv_stride..][..uv_stride],
                ],
            );
            uv_rows += 1;
        }
        pad_rows(&mut self.u, uv_stride, uv_rows);
        pad_rows(&mut self.v, uv_stride, uv_rows);
    }
}

fn luma_row<S: Simd>(simd: S, rgba: &[u8], dst: &mut [u8]) {
    let (y, pad) = dst.split_at_mut(rgba.len() / RGBA_CHANNELS);
    let blocks = y.len() / 16;
    for b in 0..blocks {
        luma_lanes(u8x16::load_four_interleaved(simd, &rgba[b * 64..][..64]))
            .store_slice(&mut y[b * 16..b * 16 + 16]);
    }
    for (px, out) in rgba[blocks * 64..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(&mut y[blocks * 16..])
    {
        *out = luma(px[0], px[1], px[2]);
    }
    pad.fill(y[y.len() - 1]);
}

#[inline(always)]
fn luma_lanes<S: Simd>([r, g, b, _]: [u8x16<S>; 4]) -> u8x16<S> {
    let (r_lo, r_hi) = r.widen();
    let (g_lo, g_hi) = g.widen();
    let (b_lo, b_hi) = b.widen();
    let lo = ((r_lo * 66 + g_lo * 129 + b_lo * 25 + 128) >> 8u32) + 16;
    let hi = ((r_hi * 66 + g_hi * 129 + b_hi * 25 + 128) >> 8u32) + 16;
    lo.narrow(hi)
}

#[inline(always)]
fn pair_sums<S: Simd>(v: u8x16<S>) -> u16x8<S> {
    let (lo, hi) = v.widen();
    lo.unzip_low(hi) + lo.unzip_high(hi)
}

#[inline(always)]
fn chroma_cells<S: Simd>(
    [tr, tg, tb, ta]: [u8x16<S>; 4],
    [br, bg, bb, ba]: [u8x16<S>; 4],
    weighted: bool,
) -> (i16x8<S>, i16x8<S>, u64) {
    let avg = |t: u8x16<S>, b: u8x16<S>| (pair_sums(t) + pair_sums(b)) >> 2u32;
    let r: i16x8<S> = avg(tr, br).bitcast();
    let g: i16x8<S> = avg(tg, bg).bitcast();
    let b: i16x8<S> = avg(tb, bb).bitcast();
    let u = ((r * -38 + g * -74 + b * 112 + 128) >> 8u32) + 128;
    let v = ((r * 112 + g * -94 + b * -18 + 128) >> 8u32) + 128;
    let mask = if weighted {
        let (tl, tr) = adjacent(ta);
        let (bl, br) = adjacent(ba);
        let lo = tl.min(tr).min(bl).min(br);
        let hi = tl.max(tr).max(bl).max(br);
        (hi - lo).simd_ge(8).to_bitmask()
    } else {
        0
    };
    (u, v, mask)
}

#[inline(always)]
fn adjacent<S: Simd>(v: u8x16<S>) -> (u16x8<S>, u16x8<S>) {
    let (lo, hi) = v.widen();
    (lo.unzip_low(hi), lo.unzip_high(hi))
}

fn row_pair<S: Simd>(
    simd: S,
    weighted: bool,
    top: &[u8],
    bot: &[u8],
    top_y: &mut [u8],
    bot_y: &mut [u8],
    [u, v]: [&mut [u8]; 2],
) {
    let w = top.len() / 4;
    let cols = w.div_ceil(2);
    let pairs = w / 2;
    let mut col = 0;
    while col + 16 <= pairs {
        let at = col * 8;
        let mut cells = |at: usize, x: usize| {
            let t = u8x16::load_four_interleaved(simd, &top[at..at + 64]);
            let b = u8x16::load_four_interleaved(simd, &bot[at..at + 64]);
            luma_lanes(t).store_slice(&mut top_y[x..x + 16]);
            luma_lanes(b).store_slice(&mut bot_y[x..x + 16]);
            chroma_cells(t, b, weighted)
        };
        let (lu, lv, lm) = cells(at, col * 2);
        let (hu, hv, hm) = cells(at + 64, col * 2 + 16);
        lu.narrow(hu)
            .bitcast::<u8x16<S>>()
            .store_slice(&mut u[col..col + 16]);
        lv.narrow(hv)
            .bitcast::<u8x16<S>>()
            .store_slice(&mut v[col..col + 16]);
        let mut mask = lm | hm << 8;
        while mask != 0 {
            let cell = col + mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let left = cell * 8;
            let (r, g, b) = cell_rgb(top, bot, left, left + 4, true);
            u[cell] = chroma_u(r, g, b);
            v[cell] = chroma_v(r, g, b);
        }
        col += 16;
    }
    if col * 2 < w {
        luma_row(simd, &top[col * 8..], &mut top_y[col * 2..]);
        luma_row(simd, &bot[col * 8..], &mut bot_y[col * 2..]);
    }
    for cell in col..pairs {
        let left = cell * 8;
        let (r, g, b) = cell_rgb(top, bot, left, left + 4, weighted);
        u[cell] = chroma_u(r, g, b);
        v[cell] = chroma_v(r, g, b);
    }
    if w % 2 == 1 {
        let last = (w - 1) * 4;
        let (r, g, b) = cell_rgb(top, bot, last, last, weighted);
        u[pairs] = chroma_u(r, g, b);
        v[pairs] = chroma_v(r, g, b);
    }
    if cols < u.len() {
        let last = top.len() - 4;
        let (r, g, b) = cell_rgb(top, bot, last, last, weighted);
        u[cols..].fill(chroma_u(r, g, b));
        v[cols..].fill(chroma_v(r, g, b));
    }
}

fn cell_rgb(top: &[u8], bot: &[u8], left: usize, right: usize, weighted: bool) -> (u8, u8, u8) {
    if weighted && let Some(rgb) = weighted_rgb(top, bot, left, right) {
        return rgb;
    }
    average_rgb(top, bot, left, right)
}

fn weighted_rgb(top: &[u8], bot: &[u8], left: usize, right: usize) -> Option<(u8, u8, u8)> {
    let px = [&top[left..], &top[right..], &bot[left..], &bot[right..]];
    let weights = px.map(|p| u32::from(p[3]));
    let [a, b, c, d] = weights;
    if a.max(b).max(c).max(d) - a.min(b).min(c).min(d) < 8 {
        return None;
    }
    let total = a + b + c + d;
    let ch = |i: usize| {
        ((px.iter()
            .zip(weights)
            .map(|(p, w)| u32::from(p[i]) * w)
            .sum::<u32>()
            + total / 2)
            / total) as u8
    };
    Some((ch(0), ch(1), ch(2)))
}

pub(crate) fn prefers_weighted_chroma(rgba: &[u8], w: usize, h: usize) -> bool {
    let cols = w.div_ceil(2);
    let rows = h.div_ceil(2);
    let px = rgba.as_chunks::<4>().0;
    let mut seed = 0x71a5_b963_u32;
    let mut varying = 0;
    let mut edges = 0;
    let mut hidden = 0;
    for _ in 0..1024 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let cell = (seed >> 8) as usize % (cols * rows);
        let x = (cell % cols) * 2;
        let y = (cell / cols) * 2;
        let right = (x + 1).min(w - 1);
        let top = y * w;
        let bot = (y + 1).min(h - 1) * w;
        let [a, b, c, d] = [top + x, top + right, bot + x, bot + right].map(|i| px[i][3]);
        let lo = a.min(b).min(c).min(d);
        let delta = a.max(b).max(c).max(d) - lo;
        hidden += usize::from(lo == 0);
        varying += usize::from(delta >= 8);
        edges += usize::from(delta >= 32);
    }
    hidden >= 32 && edges >= 8 && edges * 2 >= varying
}

fn average_rgb(top: &[u8], bot: &[u8], left: usize, right: usize) -> (u8, u8, u8) {
    let ch = |off| {
        ((u16::from(top[left + off])
            + u16::from(top[right + off])
            + u16::from(bot[left + off])
            + u16::from(bot[right + off]))
            >> 2) as u8
    };
    (ch(0), ch(1), ch(2))
}

fn pad_rows(plane: &mut [u8], stride: usize, rows: usize) {
    for row in rows..plane.len() / stride {
        plane.copy_within((rows - 1) * stride..rows * stride, row * stride);
    }
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

fn chroma_u(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8
}

fn chroma_v(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use fearless_simd::Fallback;

    #[test]
    fn bt601_endpoints() {
        let mut row = YuvRow::new(0..1);
        for (v, y) in [(255, 235), (0, 16), (128, 126)] {
            let rgba = [v, v, v, 255].repeat(16 * 16);
            let src = RgbaFrame::new(&rgba, 16, 16);
            row.load(Fallback::new(), src, 0);
            assert_eq!(row.y[0], y);
            assert_eq!(row.u[0], 128);
            assert_eq!(row.v[0], 128);
        }
    }

    #[test]
    fn macroblock_row_padding() {
        let rgba: Vec<_> = (0..18)
            .flat_map(|y| (0..18).flat_map(move |x| [x * 7, y * 7, (x + y) * 7, 255]))
            .collect();
        let src = RgbaFrame::new(&rgba, 18, 18);
        let mut row = YuvRow::new(1..2);
        row.load(Fallback::new(), src, 1);
        assert_eq!(row.y.len(), 16 * 16);
        assert_eq!(row.u.len(), 8 * 8);
        assert_eq!(row.y[15 * 16 + 15], luma(119, 119, 238));
        assert_eq!(row.u[0], chroma_u(115, 115, 231));
        assert_eq!(row.v[0], chroma_v(115, 115, 231));
        assert_eq!(row.u[7], chroma_u(119, 115, 234));
        assert_eq!(row.v[7], chroma_v(119, 115, 234));
        assert_eq!(row.u[7 * 8 + 7], chroma_u(119, 119, 238));
        assert_eq!(row.v[7 * 8 + 7], chroma_v(119, 119, 238));
    }
}
