//! BT.601 limited-range RGBA-to-YUV420 conversion with macroblock padding.

use fearless_simd::{Simd, i16x8, prelude::*, u8x16, u16x8};

/// One frame's luma and chroma planes, padded to whole macroblocks.
pub(crate) struct YuvPlanes {
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
    pub(crate) y_stride: usize,
    pub(crate) chroma_stride: usize,
}

/// Convert straight-alpha RGBA8 pixels to limited-range YUV420.
///
/// Chroma averages 2×2 RGB cells before conversion. Padding repeats source pixels before
/// averaging, so an even-width frame's right chroma padding can differ from its last cell.
pub(crate) fn from_rgba<S: Simd>(simd: S, rgba: &[u8], width: usize, height: usize) -> YuvPlanes {
    let y_stride = width.div_ceil(16) * 16;
    let y_height = height.div_ceil(16) * 16;
    let chroma_stride = width.div_ceil(16) * 8;
    let chroma_height = height.div_ceil(16) * 8;

    let mut y = vec![0u8; y_stride * y_height];
    let mut u = vec![0u8; chroma_stride * chroma_height];
    let mut v = vec![0u8; chroma_stride * chroma_height];

    for row in 0..height {
        let source = &rgba[row * width * 4..(row + 1) * width * 4];
        let destination = &mut y[row * y_stride..(row + 1) * y_stride];
        luma_row(simd, source, &mut destination[..width]);
        let last = destination[width - 1];
        destination[width..].fill(last);
    }
    pad_rows(&mut y, y_stride, height, y_height);

    let chroma_rows = height.div_ceil(2);
    for row in 0..chroma_rows {
        let top = row * 2;
        let bottom = (top + 1).min(height - 1);
        chroma_row(
            simd,
            &rgba[top * width * 4..(top + 1) * width * 4],
            &rgba[bottom * width * 4..(bottom + 1) * width * 4],
            &mut u[row * chroma_stride..(row + 1) * chroma_stride],
            &mut v[row * chroma_stride..(row + 1) * chroma_stride],
        );
    }
    if chroma_rows < chroma_height {
        let last = &rgba[(height - 1) * width * 4..height * width * 4];
        chroma_row(
            simd,
            last,
            last,
            &mut u[chroma_rows * chroma_stride..(chroma_rows + 1) * chroma_stride],
            &mut v[chroma_rows * chroma_stride..(chroma_rows + 1) * chroma_stride],
        );
        pad_rows(&mut u, chroma_stride, chroma_rows + 1, chroma_height);
        pad_rows(&mut v, chroma_stride, chroma_rows + 1, chroma_height);
    }

    YuvPlanes {
        y,
        u,
        v,
        y_stride,
        chroma_stride,
    }
}

fn luma_row<S: Simd>(simd: S, rgba: &[u8], y: &mut [u8]) {
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

fn pad_rows(plane: &mut [u8], stride: usize, rows: usize, padded_rows: usize) {
    for row in rows..padded_rows {
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

    fn solid(red: u8, green: u8, blue: u8) -> Vec<u8> {
        [red, green, blue, 255].repeat(16 * 16)
    }

    #[test]
    fn limited_range_endpoints_convert_to_their_codes() {
        let white = from_rgba(Fallback::new(), &solid(255, 255, 255), 16, 16);
        assert_eq!(white.y[0], 235);
        assert_eq!(white.u[0], 128);
        assert_eq!(white.v[0], 128);

        let black = from_rgba(Fallback::new(), &solid(0, 0, 0), 16, 16);
        assert_eq!(black.y[0], 16);
        assert_eq!(black.u[0], 128);
        assert_eq!(black.v[0], 128);

        let gray = from_rgba(Fallback::new(), &solid(128, 128, 128), 16, 16);
        assert_eq!(gray.y[0], 126);
        assert_eq!(gray.u[0], 128);
        assert_eq!(gray.v[0], 128);
    }

    #[test]
    fn planes_are_padded_to_whole_macroblocks() {
        let rgba = [10, 20, 30, 255].repeat(17 * 9);
        let planes = from_rgba(Fallback::new(), &rgba, 17, 9);
        assert_eq!(planes.y.len(), 32 * 16);
        assert_eq!(planes.y_stride, 32);
        assert_eq!(planes.u.len(), 16 * 8);
        assert_eq!(planes.chroma_stride, 16);
        assert_eq!(planes.y[0], planes.y[15 * 32 + 31]);
        assert_eq!(planes.u[0], planes.u[7 * 16 + 15]);
    }
}
