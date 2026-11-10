//! BT.601 limited-range RGBA-to-YUV420 conversion with macroblock padding.

use std::ops::Range;

use crate::macroblock::{CHROMA_SIDE, MACROBLOCK_SIDE};
use fearless_simd::{Simd, i16x8, prelude::*, u8x16, u16x8};

const RGBA_CHANNELS: usize = 4;
pub(crate) const YUV_BYTES_PER_MACROBLOCK: usize =
    MACROBLOCK_SIDE * MACROBLOCK_SIDE + 2 * CHROMA_SIDE * CHROMA_SIDE;

#[derive(Clone, Copy)]
pub(crate) struct RgbaFrame<'a> {
    rgba: &'a [u8],
    width: usize,
    height: usize,
}

impl<'a> RgbaFrame<'a> {
    pub(crate) fn new(rgba: &'a [u8], width: usize, height: usize) -> Self {
        Self {
            rgba,
            width,
            height,
        }
    }

    pub(crate) fn columns(self) -> usize {
        self.width.div_ceil(MACROBLOCK_SIDE)
    }

    pub(crate) fn rows(self) -> usize {
        self.height.div_ceil(MACROBLOCK_SIDE)
    }

    fn row(self, y: usize, (start, end): (usize, usize)) -> &'a [u8] {
        let offset = y * self.width;
        &self.rgba[(offset + start) * RGBA_CHANNELS..(offset + end) * RGBA_CHANNELS]
    }

    fn pixels(self, columns: Range<usize>) -> (usize, usize) {
        (
            columns.start * MACROBLOCK_SIDE,
            (columns.end * MACROBLOCK_SIDE).min(self.width),
        )
    }

    /// Convert just the luma of one sampled macroblock for the activity estimate.
    pub(crate) fn luma_block<S: Simd>(
        self,
        simd: S,
        x: usize,
        y: usize,
    ) -> [u8; MACROBLOCK_SIDE * MACROBLOCK_SIDE] {
        let first = y * MACROBLOCK_SIDE;
        let rows = (self.height - first).min(MACROBLOCK_SIDE);
        let pixels = self.pixels(x..x + 1);
        let mut block = [0; MACROBLOCK_SIDE * MACROBLOCK_SIDE];
        for (line, destination) in block
            .as_chunks_mut::<MACROBLOCK_SIDE>()
            .0
            .iter_mut()
            .take(rows)
            .enumerate()
        {
            luma_row(simd, self.row(first + line, pixels), destination);
        }
        pad_rows(&mut block, MACROBLOCK_SIDE, rows);
        block
    }
}

/// The source samples for one macroblock row of an analysis strip.
pub(crate) struct YuvRow {
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
    columns: Range<usize>,
}

impl YuvRow {
    pub(crate) fn new(columns: Range<usize>) -> Self {
        let width = columns.len();
        Self {
            y: vec![0; width * MACROBLOCK_SIDE * MACROBLOCK_SIDE],
            u: vec![0; width * CHROMA_SIDE * CHROMA_SIDE],
            v: vec![0; width * CHROMA_SIDE * CHROMA_SIDE],
            columns,
        }
    }

    pub(crate) fn y_stride(&self) -> usize {
        self.y.len() / MACROBLOCK_SIDE
    }

    pub(crate) fn chroma_stride(&self) -> usize {
        self.u.len() / CHROMA_SIDE
    }

    pub(crate) fn local_column(&self, x: usize) -> usize {
        x - self.columns.start
    }

    pub(crate) fn load<S: Simd>(&mut self, simd: S, frame: RgbaFrame<'_>, macroblock_y: usize) {
        let first = macroblock_y * MACROBLOCK_SIDE;
        let rows = (frame.height - first).min(MACROBLOCK_SIDE);
        let pixels = frame.pixels(self.columns.clone());
        let y_stride = self.y_stride();
        let chroma_stride = self.chroma_stride();
        let chroma_rows = rows.div_ceil(2);

        for cell in 0..chroma_rows {
            let row = cell * 2;
            let top = frame.row(first + row, pixels);
            let bottom = frame.row(first + (row + 1).min(rows - 1), pixels);
            luma_row(simd, top, &mut self.y[row * y_stride..][..y_stride]);
            if row + 1 < rows {
                luma_row(
                    simd,
                    bottom,
                    &mut self.y[(row + 1) * y_stride..][..y_stride],
                );
            }
            chroma_row(
                simd,
                top,
                bottom,
                &mut self.u[cell * chroma_stride..][..chroma_stride],
                &mut self.v[cell * chroma_stride..][..chroma_stride],
            );
        }
        pad_rows(&mut self.y, y_stride, rows);

        // When the last real cell used two distinct rows, the bottom padding instead
        // averages the final source row with itself.
        let mut chroma_rows = chroma_rows;
        if chroma_rows < CHROMA_SIDE && rows.is_multiple_of(2) {
            let last = frame.row(first + rows - 1, pixels);
            chroma_row(
                simd,
                last,
                last,
                &mut self.u[chroma_rows * chroma_stride..][..chroma_stride],
                &mut self.v[chroma_rows * chroma_stride..][..chroma_stride],
            );
            chroma_rows += 1;
        }
        pad_rows(&mut self.u, chroma_stride, chroma_rows);
        pad_rows(&mut self.v, chroma_stride, chroma_rows);
    }
}

fn luma_row<S: Simd>(simd: S, rgba: &[u8], y: &mut [u8]) {
    let (y, padding) = y.split_at_mut(rgba.len() / RGBA_CHANNELS);
    let blocks = y.len() / 16;
    for block in 0..blocks {
        let [red, green, blue, _] =
            u8x16::load_four_interleaved(simd, &rgba[block * 64..block * 64 + 64]);
        let (red_low, red_high) = red.widen();
        let (green_low, green_high) = green.widen();
        let (blue_low, blue_high) = blue.widen();
        // The largest numerator is 56,228, so every intermediate fits u16.
        let low = ((red_low * 66 + green_low * 129 + blue_low * 25 + 128) >> 8u32) + 16;
        let high = ((red_high * 66 + green_high * 129 + blue_high * 25 + 128) >> 8u32) + 16;
        low.narrow(high)
            .store_slice(&mut y[block * 16..block * 16 + 16]);
    }
    for (pixel, destination) in rgba[blocks * 64..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(&mut y[blocks * 16..])
    {
        *destination = luma(pixel[0], pixel[1], pixel[2]);
    }
    padding.fill(y[y.len() - 1]);
}

#[inline(always)]
fn pair_sums<S: Simd>(values: u8x16<S>) -> u16x8<S> {
    let (low, high) = values.widen();
    low.unzip_low(high) + low.unzip_high(high)
}

#[inline(always)]
fn chroma_cells<S: Simd>(simd: S, top: &[u8], bottom: &[u8]) -> (i16x8<S>, i16x8<S>) {
    let [top_red, top_green, top_blue, _] = u8x16::load_four_interleaved(simd, top);
    let [bottom_red, bottom_green, bottom_blue, _] = u8x16::load_four_interleaved(simd, bottom);
    let average = |top: u8x16<S>, bottom: u8x16<S>| (pair_sums(top) + pair_sums(bottom)) >> 2u32;
    // Limited-range BT.601 chroma needs no saturation; the sums stay within i16.
    let red: i16x8<S> = average(top_red, bottom_red).bitcast();
    let green: i16x8<S> = average(top_green, bottom_green).bitcast();
    let blue: i16x8<S> = average(top_blue, bottom_blue).bitcast();
    let u = ((red * -38 + green * -74 + blue * 112 + 128) >> 8u32) + 128;
    let v = ((red * 112 + green * -94 + blue * -18 + 128) >> 8u32) + 128;
    (u, v)
}

fn chroma_row<S: Simd>(simd: S, top: &[u8], bottom: &[u8], u: &mut [u8], v: &mut [u8]) {
    let width = top.len() / 4;
    let columns = width.div_ceil(2);
    let pairs = width / 2;
    let mut column = 0;
    while column + 16 <= pairs {
        let at = column * 8;
        let (low_u, low_v) = chroma_cells(simd, &top[at..at + 64], &bottom[at..at + 64]);
        let (high_u, high_v) =
            chroma_cells(simd, &top[at + 64..at + 128], &bottom[at + 64..at + 128]);
        low_u
            .narrow(high_u)
            .bitcast::<u8x16<S>>()
            .store_slice(&mut u[column..column + 16]);
        low_v
            .narrow(high_v)
            .bitcast::<u8x16<S>>()
            .store_slice(&mut v[column..column + 16]);
        column += 16;
    }
    for cell in column..pairs {
        let left = cell * 8;
        let (red, green, blue) = average_rgb(top, bottom, left, left + 4);
        u[cell] = chroma_u(red, green, blue);
        v[cell] = chroma_v(red, green, blue);
    }
    if width % 2 == 1 {
        let last = (width - 1) * 4;
        let (red, green, blue) = average_rgb(top, bottom, last, last);
        u[pairs] = chroma_u(red, green, blue);
        v[pairs] = chroma_v(red, green, blue);
    }
    if columns < u.len() {
        // Padding repeats the source's last pixel, not its last 2×2 cell.
        let last = top.len() - 4;
        let (red, green, blue) = average_rgb(top, bottom, last, last);
        u[columns..].fill(chroma_u(red, green, blue));
        v[columns..].fill(chroma_v(red, green, blue));
    }
}

fn average_rgb(top: &[u8], bottom: &[u8], left: usize, right: usize) -> (u8, u8, u8) {
    let channel = |offset| {
        ((u16::from(top[left + offset])
            + u16::from(top[right + offset])
            + u16::from(bottom[left + offset])
            + u16::from(bottom[right + offset]))
            >> 2) as u8
    };
    (channel(0), channel(1), channel(2))
}

fn pad_rows(plane: &mut [u8], stride: usize, rows: usize) {
    for row in rows..plane.len() / stride {
        plane.copy_within((rows - 1) * stride..rows * stride, row * stride);
    }
}

/// Limited-range BT.601 luma.
fn luma(red: u8, green: u8, blue: u8) -> u8 {
    let (r, g, b) = (i32::from(red), i32::from(green), i32::from(blue));
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

/// Limited-range BT.601 U (blue difference).
fn chroma_u(red: u8, green: u8, blue: u8) -> u8 {
    let (r, g, b) = (i32::from(red), i32::from(green), i32::from(blue));
    (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8
}

/// Limited-range BT.601 V (red difference).
fn chroma_v(red: u8, green: u8, blue: u8) -> u8 {
    let (r, g, b) = (i32::from(red), i32::from(green), i32::from(blue));
    (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use fearless_simd::Fallback;

    #[test]
    fn limited_range_endpoints_convert_to_their_codes() {
        let mut samples = YuvRow::new(0..1);
        for (value, y) in [(255, 235), (0, 16), (128, 126)] {
            let rgba = [value, value, value, 255].repeat(16 * 16);
            let source = RgbaFrame::new(&rgba, 16, 16);
            samples.load(Fallback::new(), source, 0);
            assert_eq!(samples.y[0], y);
            assert_eq!(samples.u[0], 128);
            assert_eq!(samples.v[0], 128);
        }
    }

    #[test]
    fn planes_are_padded_to_whole_macroblocks() {
        let rgba: Vec<_> = (0..18)
            .flat_map(|y| (0..18).flat_map(move |x| [x * 7, y * 7, (x + y) * 7, 255]))
            .collect();
        let source = RgbaFrame::new(&rgba, 18, 18);
        let mut samples = YuvRow::new(1..2);
        samples.load(Fallback::new(), source, 1);
        assert_eq!(samples.y.len(), 16 * 16);
        assert_eq!(samples.u.len(), 8 * 8);
        assert_eq!(samples.y[15 * 16 + 15], luma(119, 119, 238));
        assert_eq!(samples.u[0], chroma_u(115, 115, 231));
        assert_eq!(samples.v[0], chroma_v(115, 115, 231));
        assert_eq!(samples.u[7], chroma_u(119, 115, 234));
        assert_eq!(samples.v[7], chroma_v(119, 115, 234));
        assert_eq!(samples.u[7 * 8 + 7], chroma_u(119, 119, 238));
        assert_eq!(samples.v[7 * 8 + 7], chroma_v(119, 119, 238));
    }
}
