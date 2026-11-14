//! Extend visible colours through fully transparent pixels before VP8 conversion.

use crate::Error;
use crate::encoder::check_cancelled;

const MAX_FLAT_PIXELS: usize = 512 * 1024;
const MAX_PIXELS: usize = 1024 * 1024;
const MAX_COLOURS: usize = 8;

pub(crate) fn scratch_bytes(pixels: u64) -> u64 {
    if pixels <= MAX_PIXELS as u64 {
        pixels * 4
    } else {
        0
    }
}

pub(crate) fn extend(
    rgba: &[u8],
    width: usize,
    height: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let pixels = rgba.as_chunks::<4>().0;
    if pixels.len() > MAX_PIXELS {
        return Ok(None);
    }

    if mostly_translucent(pixels) {
        if faint_border(pixels) {
            return soften_faint_pixels(rgba, width, height, cancelled).map(Some);
        }
        return fill_rows(rgba, width, cancelled).map(Some);
    }
    if pixels.len() > MAX_FLAT_PIXELS {
        return Ok(None);
    }

    // Use opaque samples to identify flat artwork. Antialiased pixels may have
    // intermediate colours even when the artwork has only a small palette.
    let mut colours = [[0; 3]; MAX_COLOURS];
    let mut count = 0;
    let mut transparent = false;
    let mut translucent = 0;
    for row in pixels.chunks_exact(width) {
        check_cancelled(cancelled)?;
        for &[r, g, b, alpha] in row {
            transparent |= alpha == 0;
            translucent += usize::from(alpha != 0 && alpha != 255);
            if alpha == 255 {
                let rgb = [r, g, b];
                if !colours[..count].contains(&rgb) {
                    if count == MAX_COLOURS {
                        return match neutral_background(pixels) {
                            Some(gray) => {
                                fill_cells(rgba, width, height, gray, cancelled).map(Some)
                            }
                            None => Ok(None),
                        };
                    }
                    colours[count] = rgb;
                    count += 1;
                }
            }
        }
    }
    if !transparent || count == 0 {
        return Ok(None);
    }

    // Allow a few antialiased outliers. A translucent region with its own
    // palette is a different case and keeps the ordinary colour conversion.
    if translucent != 0 {
        let mut outliers = 0;
        for row in pixels.chunks_exact(width) {
            check_cancelled(cancelled)?;
            for &[r, g, b, alpha] in row {
                if alpha != 0
                    && alpha != 255
                    && !colours[..count].iter().any(|colour| {
                        [r, g, b]
                            .into_iter()
                            .zip(colour)
                            .all(|(a, &b)| a.abs_diff(b) <= 4)
                    })
                {
                    outliers += 1;
                    if outliers > translucent / 50 {
                        return Ok(None);
                    }
                }
            }
        }
    }

    let mut filled = rgba.to_vec();
    let output = filled.as_chunks_mut::<4>().0;
    let mut previous_column = None;
    for x in 0..width {
        check_cancelled(cancelled)?;
        let mut above = None;
        for y in 0..height {
            let index = y * width + x;
            if pixels[index][3] == 0 {
                continue;
            }
            let start = above.map_or(0, |above| above + 1);
            for row in start..y {
                let nearest = above.filter(|&above| row - above <= y - row).unwrap_or(y);
                output[row * width + x][..3].copy_from_slice(&pixels[nearest * width + x][..3]);
            }
            above = Some(y);
        }
        if let Some(above) = above {
            let rgb = &pixels[above * width + x][..3];
            for row in above + 1..height {
                output[row * width + x][..3].copy_from_slice(rgb);
            }
            let start = previous_column.map_or(0, |left| left + 1);
            for empty in start..x {
                let nearest = previous_column
                    .filter(|&left| empty - left <= x - empty)
                    .unwrap_or(x);
                copy_column(output, width, nearest, empty, cancelled)?;
            }
            previous_column = Some(x);
        }
    }
    if let Some(left) = previous_column {
        for empty in left + 1..width {
            copy_column(output, width, left, empty, cancelled)?;
        }
    }
    Ok(Some(filled))
}

fn mostly_translucent(pixels: &[[u8; 4]]) -> bool {
    const SAMPLES: usize = 256;
    if pixels.len() < 16 * 1024 {
        return false;
    }
    let mut seed = 0x71a5_b963u32;
    let mut hidden = 0;
    let mut opaque = 0;
    for _ in 0..SAMPLES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let alpha = pixels[(seed >> 8) as usize % pixels.len()][3];
        hidden += usize::from(alpha == 0);
        opaque += usize::from(alpha == 255);
        if opaque > SAMPLES / 16 {
            return false;
        }
    }
    hidden >= SAMPLES / 8 && opaque + hidden <= SAMPLES * 7 / 8
}

// Low-opacity skirts can retain fine RGB texture that dominates the colour
// stream while contributing little to the composite.
fn faint_border(pixels: &[[u8; 4]]) -> bool {
    const SAMPLES: usize = 512;
    let mut seed = 0xa746_5c19u32;
    let mut faint = 0;
    for _ in 0..SAMPLES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let alpha = pixels[(seed >> 8) as usize % pixels.len()][3];
        if alpha >= 240 {
            return false;
        }
        faint += usize::from((1..=16).contains(&alpha));
    }
    faint >= SAMPLES / 20
}

fn soften_faint_pixels(
    rgba: &[u8],
    width: usize,
    height: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    const CUTOFF: u32 = 56;
    const DIVISOR: u32 = CUTOFF * 16;
    const WEIGHTS: [u32; 3] = [1, 2, 1];

    let input = rgba.as_chunks::<4>().0;
    let mut filled = rgba.to_vec();
    for (y, row) in filled
        .as_chunks_mut::<4>()
        .0
        .chunks_exact_mut(width)
        .enumerate()
    {
        check_cancelled(cancelled)?;
        let rows = [y.saturating_sub(1), y, (y + 1).min(height - 1)].map(|row| row * width);
        for (x, pixel) in row.iter_mut().enumerate() {
            let alpha = u32::from(pixel[3]);
            if alpha == 0 {
                pixel[..3].fill(128);
            } else if alpha < CUTOFF {
                let columns = [x.saturating_sub(1), x, (x + 1).min(width - 1)];
                let mut sum = [0; 3];
                for (row, wy) in rows.into_iter().zip(WEIGHTS) {
                    for (column, wx) in columns.into_iter().zip(WEIGHTS) {
                        for (total, value) in sum.iter_mut().zip(input[row + column]) {
                            *total += u32::from(value) * wy * wx;
                        }
                    }
                }
                for (channel, filtered) in pixel[..3].iter_mut().zip(sum) {
                    let weighted = u32::from(*channel) * alpha * 16 + filtered * (CUTOFF - alpha);
                    *channel = ((weighted + DIVISOR / 2) / DIVISOR) as u8;
                }
            }
        }
    }
    Ok(filled)
}

/// Flatten a dominant neutral background in artwork with sparse visible pixels.
fn neutral_background(pixels: &[[u8; 4]]) -> Option<u8> {
    const SAMPLES: usize = 512;
    if pixels.len() < 16 * 1024 {
        return None;
    }
    let mut seed = 0x71a5_b963u32;
    let mut hidden = 0;
    let mut histogram = [0; 256];
    for _ in 0..SAMPLES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [r, g, b, a] = pixels[(seed >> 8) as usize % pixels.len()];
        if a == 0 {
            hidden += 1;
            if r == g && g == b {
                histogram[r as usize] += 1;
            }
        }
    }
    let (gray, count) = histogram
        .into_iter()
        .enumerate()
        .max_by_key(|&(_, count)| count)?;
    (hidden * 3 >= SAMPLES * 2 && count * 6 >= hidden).then_some(gray as u8)
}

fn fill_cells(
    rgba: &[u8],
    width: usize,
    height: usize,
    gray: u8,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    let mut filled = rgba.to_vec();
    let input = rgba.as_chunks::<4>().0;
    let output = filled.as_chunks_mut::<4>().0;
    for y in (0..height).step_by(2) {
        check_cancelled(cancelled)?;
        let bottom = (y + 1).min(height - 1);
        for x in (0..width).step_by(2) {
            let right = (x + 1).min(width - 1);
            let cell = [
                y * width + x,
                y * width + right,
                bottom * width + x,
                bottom * width + right,
            ];
            if cell.iter().all(|&at| input[at][3] != 0) {
                continue;
            }
            let mut rgb = [0; 3];
            let mut weight = 0;
            for at in cell {
                let [r, g, b, alpha] = input[at].map(u32::from);
                weight += alpha;
                for (total, value) in rgb.iter_mut().zip([r, g, b]) {
                    *total += value * alpha;
                }
            }
            let colour = if weight == 0 {
                [gray; 3]
            } else {
                rgb.map(|value| ((value + weight / 2) / weight) as u8)
            };
            for at in cell {
                if input[at][3] == 0 {
                    output[at][..3].copy_from_slice(&colour);
                }
            }
        }
    }
    Ok(filled)
}

fn fill_rows(
    rgba: &[u8],
    width: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    let mut filled = rgba.to_vec();
    for row in filled.as_chunks_mut::<4>().0.chunks_exact_mut(width) {
        check_cancelled(cancelled)?;
        let mut left = None;
        for right in 0..width {
            if row[right][3] == 0 {
                continue;
            }
            let rgb = [row[right][0], row[right][1], row[right][2]];
            let Some(previous) = left.replace(right) else {
                for pixel in &mut row[..right] {
                    pixel[..3].copy_from_slice(&rgb);
                }
                continue;
            };
            if right - previous > 1 {
                let midpoint = previous + (right - previous) / 2;
                let [r, g, b, _] = row[previous];
                for pixel in &mut row[previous + 1..midpoint + 1] {
                    pixel[..3].copy_from_slice(&[r, g, b]);
                }
                for pixel in &mut row[midpoint + 1..right] {
                    pixel[..3].copy_from_slice(&rgb);
                }
            }
        }
        if let Some(left) = left {
            let [r, g, b, _] = row[left];
            for pixel in &mut row[left + 1..] {
                pixel[..3].copy_from_slice(&[r, g, b]);
            }
        }
    }
    Ok(filled)
}

fn copy_column(
    pixels: &mut [[u8; 4]],
    width: usize,
    from: usize,
    to: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    check_cancelled(cancelled)?;
    for row in pixels.chunks_exact_mut(width) {
        let [r, g, b, _] = row[from];
        row[to][..3].copy_from_slice(&[r, g, b]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extends_colour_without_changing_alpha_or_visible_pixels() {
        let hidden = [255, 0, 0, 0];
        let ink = [4, 80, 160, 255];
        let edge = [4, 80, 160, 128];
        let rgba = [hidden, hidden, hidden, ink, hidden, hidden, edge, hidden].concat();
        let filled = extend(&rgba, 2, 4, &|| false).unwrap().unwrap();
        for (before, after) in rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(filled.as_chunks::<4>().0)
        {
            if before[3] == 0 {
                assert_eq!(*after, [4, 80, 160, 0]);
            } else {
                assert_eq!(before, after);
            }
        }
        let mut other = rgba;
        other[6 * 4] = 255;
        assert!(extend(&other, 2, 4, &|| false).unwrap().is_none());
    }
}
