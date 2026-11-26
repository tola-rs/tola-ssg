//! Extend visible RGB into fully transparent pixels before VP8 conversion.

use crate::Error;
use crate::encoder::check_cancelled;

const MAX_FLAT_PIXELS: usize = 512 * 1024;
const MAX_PIXELS: usize = 1024 * 1024;
const MAX_COLOURS: usize = 8;

pub(crate) fn scratch_bytes(px: u64) -> u64 {
    if px <= MAX_PIXELS as u64 { px * 4 } else { 0 }
}

pub(crate) fn extend(
    rgba: &[u8],
    w: usize,
    h: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let px = rgba.as_chunks::<4>().0;
    if px.len() > MAX_PIXELS {
        return Ok(None);
    }

    if mostly_translucent(px) {
        if faint_border(px) {
            return soften_faint_px(rgba, w, h, cancel).map(Some);
        }
        return fill_rows(rgba, w, cancel).map(Some);
    }
    if px.len() > MAX_FLAT_PIXELS {
        return Ok(None);
    }

    // Identify flat artwork by opaque colors; antialiased edges may mix intermediates.
    let mut palette = [[0; 3]; MAX_COLOURS];
    let mut count = 0;
    let mut has_zero = false;
    let mut semi = 0;
    for row in px.chunks_exact(w) {
        check_cancelled(cancel)?;
        for &[r, g, b, alpha] in row {
            has_zero |= alpha == 0;
            semi += usize::from(alpha != 0 && alpha != 255);
            if alpha == 255 {
                let rgb = [r, g, b];
                if !palette[..count].contains(&rgb) {
                    if count == MAX_COLOURS {
                        return match neutral_background(px) {
                            Some(gray) => fill_cells(rgba, w, h, gray, cancel).map(Some),
                            None => Ok(None),
                        };
                    }
                    palette[count] = rgb;
                    count += 1;
                }
            }
        }
    }
    if !has_zero || count == 0 {
        return Ok(None);
    }

    if semi != 0 {
        let mut outliers = 0;
        for row in px.chunks_exact(w) {
            check_cancelled(cancel)?;
            for &[r, g, b, alpha] in row {
                if alpha != 0
                    && alpha != 255
                    && !palette[..count].iter().any(|c| {
                        [r, g, b]
                            .into_iter()
                            .zip(c)
                            .all(|(a, &b)| a.abs_diff(b) <= 4)
                    })
                {
                    outliers += 1;
                    if outliers > semi / 50 {
                        return Ok(None);
                    }
                }
            }
        }
    }

    let mut buf = rgba.to_vec();
    let dst = buf.as_chunks_mut::<4>().0;
    let mut prev_col = None;
    for x in 0..w {
        check_cancelled(cancel)?;
        let mut above = None;
        for y in 0..h {
            let i = y * w + x;
            if px[i][3] == 0 {
                continue;
            }
            let start = above.map_or(0, |a| a + 1);
            for row in start..y {
                let nearest = above.filter(|&a| row - a <= y - row).unwrap_or(y);
                dst[row * w + x][..3].copy_from_slice(&px[nearest * w + x][..3]);
            }
            above = Some(y);
        }
        if let Some(above) = above {
            let rgb = &px[above * w + x][..3];
            for row in above + 1..h {
                dst[row * w + x][..3].copy_from_slice(rgb);
            }
            let start = prev_col.map_or(0, |left| left + 1);
            for empty in start..x {
                let nearest = prev_col.filter(|&l| empty - l <= x - empty).unwrap_or(x);
                copy_col(dst, w, nearest, empty, cancel)?;
            }
            prev_col = Some(x);
        }
    }
    if let Some(left) = prev_col {
        for empty in left + 1..w {
            copy_col(dst, w, left, empty, cancel)?;
        }
    }
    Ok(Some(buf))
}

fn lcg_next(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *seed >> 8
}

fn mostly_translucent(px: &[[u8; 4]]) -> bool {
    const SAMPLES: usize = 256;
    if px.len() < 16 * 1024 {
        return false;
    }
    let mut seed = 0x71a5_b963u32;
    let mut hidden = 0;
    let mut opaque = 0;
    for _ in 0..SAMPLES {
        let alpha = px[lcg_next(&mut seed) as usize % px.len()][3];
        hidden += usize::from(alpha == 0);
        opaque += usize::from(alpha == 255);
        if opaque > SAMPLES / 16 {
            return false;
        }
    }
    hidden >= SAMPLES / 8 && opaque + hidden <= SAMPLES * 7 / 8
}

fn faint_border(px: &[[u8; 4]]) -> bool {
    const SAMPLES: usize = 512;
    let mut seed = 0xa746_5c19u32;
    let mut faint = 0;
    for _ in 0..SAMPLES {
        let alpha = px[lcg_next(&mut seed) as usize % px.len()][3];
        if alpha >= 240 {
            return false;
        }
        faint += usize::from((1..=16).contains(&alpha));
    }
    faint >= SAMPLES / 20
}

fn soften_faint_px(
    rgba: &[u8],
    w: usize,
    h: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    const CUTOFF: u32 = 56;
    const DIVISOR: u32 = CUTOFF * 16;
    const WEIGHTS: [u32; 3] = [1, 2, 1];

    let src = rgba.as_chunks::<4>().0;
    let mut buf = rgba.to_vec();
    for (y, row) in buf.as_chunks_mut::<4>().0.chunks_exact_mut(w).enumerate() {
        check_cancelled(cancel)?;
        let rows = [y.saturating_sub(1), y, (y + 1).min(h - 1)].map(|r| r * w);
        for (x, px) in row.iter_mut().enumerate() {
            let alpha = u32::from(px[3]);
            if alpha == 0 {
                px[..3].fill(128);
            } else if alpha < CUTOFF {
                let cols = [x.saturating_sub(1), x, (x + 1).min(w - 1)];
                let mut sum = [0; 3];
                for (r, wy) in rows.into_iter().zip(WEIGHTS) {
                    for (c, wx) in cols.into_iter().zip(WEIGHTS) {
                        for (total, v) in sum.iter_mut().zip(src[r + c]) {
                            *total += u32::from(v) * wy * wx;
                        }
                    }
                }
                for (ch, filtered) in px[..3].iter_mut().zip(sum) {
                    let weighted = u32::from(*ch) * alpha * 16 + filtered * (CUTOFF - alpha);
                    *ch = ((weighted + DIVISOR / 2) / DIVISOR) as u8;
                }
            }
        }
    }
    Ok(buf)
}

fn neutral_background(px: &[[u8; 4]]) -> Option<u8> {
    const SAMPLES: usize = 512;
    if px.len() < 16 * 1024 {
        return None;
    }
    let mut seed = 0x71a5_b963u32;
    let mut hidden = 0;
    let mut hist = [0; 256];
    for _ in 0..SAMPLES {
        let [r, g, b, a] = px[lcg_next(&mut seed) as usize % px.len()];
        if a == 0 {
            hidden += 1;
            if r == g && g == b {
                hist[r as usize] += 1;
            }
        }
    }
    let (gray, count) = hist.into_iter().enumerate().max_by_key(|&(_, n)| n)?;
    (hidden * 3 >= SAMPLES * 2 && count * 6 >= hidden).then_some(gray as u8)
}

fn fill_cells(
    rgba: &[u8],
    w: usize,
    h: usize,
    gray: u8,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    let mut buf = rgba.to_vec();
    let src = rgba.as_chunks::<4>().0;
    let dst = buf.as_chunks_mut::<4>().0;
    for y in (0..h).step_by(2) {
        check_cancelled(cancel)?;
        let bot = (y + 1).min(h - 1);
        for x in (0..w).step_by(2) {
            let right = (x + 1).min(w - 1);
            let cell = [y * w + x, y * w + right, bot * w + x, bot * w + right];
            if cell.iter().all(|&at| src[at][3] != 0) {
                continue;
            }
            let mut rgb = [0; 3];
            let mut weight = 0;
            for at in cell {
                let [r, g, b, alpha] = src[at].map(u32::from);
                weight += alpha;
                for (total, v) in rgb.iter_mut().zip([r, g, b]) {
                    *total += v * alpha;
                }
            }
            let colour = if weight == 0 {
                [gray; 3]
            } else {
                rgb.map(|v| ((v + weight / 2) / weight) as u8)
            };
            for at in cell {
                if src[at][3] == 0 {
                    dst[at][..3].copy_from_slice(&colour);
                }
            }
        }
    }
    Ok(buf)
}

fn fill_rows(rgba: &[u8], w: usize, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Vec<u8>, Error> {
    let mut buf = rgba.to_vec();
    for row in buf.as_chunks_mut::<4>().0.chunks_exact_mut(w) {
        check_cancelled(cancel)?;
        let mut left = None;
        for right in 0..w {
            if row[right][3] == 0 {
                continue;
            }
            let rgb = [row[right][0], row[right][1], row[right][2]];
            let Some(prev) = left.replace(right) else {
                for px in &mut row[..right] {
                    px[..3].copy_from_slice(&rgb);
                }
                continue;
            };
            if right - prev > 1 {
                let mid = prev + (right - prev) / 2;
                let [r, g, b, _] = row[prev];
                for px in &mut row[prev + 1..mid + 1] {
                    px[..3].copy_from_slice(&[r, g, b]);
                }
                for px in &mut row[mid + 1..right] {
                    px[..3].copy_from_slice(&rgb);
                }
            }
        }
        if let Some(left) = left {
            let [r, g, b, _] = row[left];
            for px in &mut row[left + 1..] {
                px[..3].copy_from_slice(&[r, g, b]);
            }
        }
    }
    Ok(buf)
}

fn copy_col(
    px: &mut [[u8; 4]],
    w: usize,
    from: usize,
    to: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    check_cancelled(cancel)?;
    for row in px.chunks_exact_mut(w) {
        let [r, g, b, _] = row[from];
        row[to][..3].copy_from_slice(&[r, g, b]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_transparent_rgb() {
        let hidden = [255, 0, 0, 0];
        let ink = [4, 80, 160, 255];
        let edge = [4, 80, 160, 128];
        let rgba = [hidden, hidden, hidden, ink, hidden, hidden, edge, hidden].concat();
        let buf = extend(&rgba, 2, 4, &|| false).unwrap().unwrap();
        for (before, after) in rgba.as_chunks::<4>().0.iter().zip(buf.as_chunks::<4>().0) {
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
