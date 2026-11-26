//! VP8L entropy groups for alpha residuals with varying local distributions.

#[cfg(test)]
use super::Stream;
use super::repeated::{
    Bits, DISTANCE_SYMBOLS, GREEN_SYMBOLS, PLANE_CODES, code_lengths, codes, padded_len, prefix,
    symbol_bits,
};
use crate::{Error, encoder::check_cancelled};

const TILE: usize = 16;
const GROUPS: usize = 5;

pub(super) struct Tiles {
    map: Vec<u8>,
    ngroups: usize,
    hist: [u32; GREEN_SYMBOLS],
    side: usize,
}

impl Tiles {
    pub(super) fn single(len: usize, w: usize) -> Self {
        let count = w.div_ceil(TILE) * (len / w).div_ceil(TILE);
        let mut hist = [0; GREEN_SYMBOLS];
        hist[0] = count as u32;
        Self {
            map: vec![0; count],
            ngroups: 1,
            hist,
            side: TILE,
        }
    }
}

pub(super) fn classify(
    src: &[u8],
    w: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Tiles>, Error> {
    let h = src.len() / w;
    let cols = w.div_ceil(TILE);
    let rows = h.div_ceil(TILE);
    let mut map = vec![0u8; cols * rows];
    let mut sizes = [0usize; 3];
    for (ty, tiles) in map.chunks_exact_mut(cols).enumerate() {
        check_cancelled(cancel)?;
        for (tx, group) in tiles.iter_mut().enumerate() {
            let y0 = ty * TILE;
            let x0 = tx * TILE;
            let mut total = 0;
            let mut irregular = 0;
            for y in (y0..(y0 + TILE).min(h)).step_by(4) {
                let row = &src[y * w..][..w];
                for &v in row[x0..(x0 + TILE).min(w)].iter().step_by(4) {
                    total += 1;
                    irregular += usize::from(v.min(v.wrapping_neg()) > 2);
                }
            }
            *group = if irregular * 8 <= total {
                0
            } else if irregular * 2 <= total {
                1
            } else {
                2
            };
            sizes[*group as usize] += 1;
        }
    }
    let mut ranked = sizes;
    ranked.sort_unstable();
    if ranked[1] * 32 < map.len() {
        return Ok(None);
    }
    let mut ngroups = 0;
    let remap = sizes.map(|n| {
        let g = ngroups;
        ngroups += u8::from(n != 0);
        g
    });
    let mut hist = [0; GREEN_SYMBOLS];
    for g in &mut map {
        *g = remap[*g as usize];
        hist[*g as usize] += 1;
    }
    Ok(Some(Tiles {
        map,
        ngroups: ngroups as usize,
        hist,
        side: TILE,
    }))
}

pub(super) struct Codes {
    bits: Bits,
    green: [[(u32, u32); GREEN_SYMBOLS]; GROUPS],
    dist: [[(u32, u32); DISTANCE_SYMBOLS]; GROUPS],
    pub(super) len: usize,
}

pub(super) fn codes_for(
    tiles: &Tiles,
    src: &[u8],
    w: usize,
    mut bits: Bits,
    alpha: u32,
    copies: &[(u32, u16)],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Codes, Error> {
    let Tiles {
        map,
        ngroups,
        hist,
        side,
    } = tiles;
    let mut green = [[0; GREEN_SYMBOLS]; GROUPS];
    let mut dist = [[0; DISTANCE_SYMBOLS]; GROUPS];
    let mut extra = 0;
    visit(src, w, copies, map, *side, cancel, |g, tok| {
        match tok {
            Token::Literals(vals) => {
                for &v in vals {
                    green[g][v as usize] += 1;
                }
            }
            Token::Copy(packed) => {
                let (l, d) = copy_prefixes(packed);
                green[g][256 + l.0 as usize] += 1;
                dist[g][d.0 as usize] += 1;
                extra += (l.1 + d.1) as usize;
            }
        }
        Ok(())
    })?;
    bits.write(if *ngroups == 1 { 0 } else { 2 }, 2);
    if *ngroups != 1 {
        bits.write(side.ilog2() - 2, 3);
        bits.write(0, 1); // no color cache
        let map_codes = bits.write_green_codes(hist);
        for _ in 0..4 {
            bits.write_single_tree(0);
        }
        bits.write_literals(map, &map_codes, cancel)?;
    }
    let mut green_codes = [[(0, 0); GREEN_SYMBOLS]; GROUPS];
    let mut dist_codes = [[(0, 0); DISTANCE_SYMBOLS]; GROUPS];
    for g in 0..*ngroups {
        green_codes[g] = bits.write_green_codes(&green[g]);
        for v in [0, 0, alpha] {
            bits.write_single_tree(v);
        }
        dist_codes[g] = bits.write_distance_tree(&dist[g]);
        extra += symbol_bits(&green_codes[g], &green[g]) + symbol_bits(&dist_codes[g], &dist[g]);
    }
    let len = padded_len(bits.len() + extra);
    Ok(Codes {
        bits,
        green: green_codes,
        dist: dist_codes,
        len,
    })
}

impl Codes {
    pub(super) fn write(
        self,
        tiles: &Tiles,
        src: &[u8],
        w: usize,
        copies: &[(u32, u16)],
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        let Self {
            mut bits,
            green: green_codes,
            dist: dist_codes,
            len,
        } = self;
        bits.reserve_bytes(len);
        visit(src, w, copies, &tiles.map, tiles.side, cancel, |g, tok| {
            match tok {
                Token::Literals(vals) => {
                    for &v in vals {
                        let (code, len) = green_codes[g][v as usize];
                        bits.write(code, len);
                    }
                }
                Token::Copy(packed) => {
                    let (l, d) = copy_prefixes(packed);
                    bits.write_token(green_codes[g][256 + l.0 as usize], l.2, l.1);
                    bits.write_token(dist_codes[g][d.0 as usize], d.2, d.1);
                }
            }
            Ok(())
        })?;
        Ok(bits.finish())
    }
}

impl Bits {
    pub(super) fn write_green_codes(
        &mut self,
        hist: &[u32; GREEN_SYMBOLS],
    ) -> [(u32, u32); GREEN_SYMBOLS] {
        if let Some(lens) = code_lengths(hist, 15) {
            self.write_green_tree(&lens);
            codes(&lens)
        } else {
            let sym = hist.iter().position(|&n| n != 0).unwrap_or(0);
            if sym < 256 {
                self.write_single_tree(sym as u32);
            } else {
                let mut lens = [0; GREEN_SYMBOLS];
                lens[sym] = 1;
                self.write_green_tree(&lens);
            }
            [(0, 0); GREEN_SYMBOLS]
        }
    }
}

enum Token<'a> {
    Literals(&'a [u8]),
    Copy(u16),
}

fn visit(
    src: &[u8],
    w: usize,
    mut copies: &[(u32, u16)],
    map: &[u8],
    side: usize,
    cancel: &(dyn Fn() -> bool + Sync),
    mut write: impl FnMut(usize, Token<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    let cols = w.div_ceil(side);
    let mut at = 0;
    let mut check_at = 0;
    while at < src.len() {
        if at >= check_at {
            check_cancelled(cancel)?;
            check_at = at + 4096;
        }
        let y = at / w;
        let x = at % w;
        let g = map[(y / side) * cols + x / side] as usize;
        if let Some((&copy, rest)) = copies.split_first()
            && at == copy.0 as usize
        {
            // Spanning copies belong to the group of their starting pixel.
            write(g, Token::Copy(copy.1))?;
            at += usize::from(copy.1 & 4095) + 1;
            copies = rest;
            continue;
        }
        let end = at + (side - x % side).min(w - x);
        let end = copies
            .first()
            .map_or(end, |&(start, _)| end.min(start as usize));
        write(g, Token::Literals(&src[at..end]))?;
        at = end;
    }
    Ok(())
}

fn copy_prefixes(packed: u16) -> ((u32, u32, u32), (u32, u32, u32)) {
    (
        prefix(u32::from(packed & 4095) + 1),
        prefix(PLANE_CODES[usize::from(packed >> 12)]),
    )
}

pub(super) fn squared_residuals(
    src: &[u8],
    w: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Tiles, Error> {
    let h = src.len() / w;
    let mut map = Vec::with_capacity(w.div_ceil(TILE) * h.div_ceil(TILE));
    let mut sizes = [0; 5];
    for y0 in (0..h).step_by(TILE) {
        check_cancelled(cancel)?;
        for x0 in (0..w).step_by(TILE) {
            let mut total = 0;
            let mut count = 0;
            for y in (y0..(y0 + TILE).min(h)).step_by(2) {
                for &v in src[y * w + x0..y * w + (x0 + TILE).min(w)]
                    .iter()
                    .step_by(2)
                {
                    let a = u32::from(v.min(v.wrapping_neg()));
                    total += a * a;
                    count += 1;
                }
            }
            let g = [2, 10, 30, 100].partition_point(|&bound| total > bound * count);
            map.push(g as u8);
            sizes[g] += 1;
        }
    }
    let mut ngroups = 0u8;
    let mut hist = [0; GREEN_SYMBOLS];
    let remap = sizes.map(|n| {
        let g = ngroups;
        if n != 0 {
            hist[g as usize] = n;
            ngroups += 1;
        }
        g
    });
    for g in &mut map {
        *g = remap[*g as usize];
    }
    Ok(Tiles {
        map,
        ngroups: usize::from(ngroups),
        hist,
        side: TILE,
    })
}

pub(super) fn clustered(
    src: &[u8],
    w: usize,
    copies: &[(u32, u16)],
    side: usize,
    count: usize,
    rounds: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Tiles, Error> {
    const BINS: usize = 17;
    const BOUNDS: [u8; 15] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96];
    let cols = w.div_ceil(side);
    let ntiles = cols * (src.len() / w).div_ceil(side);
    let mut hist = vec![[0u16; BINS]; ntiles];
    let mut avg = vec![(0u32, 0u32); ntiles];
    let tile_of = |at: usize| (at / w / side) * cols + (at % w / side);
    let signed = side == TILE;
    let bins: [usize; 256] = std::array::from_fn(|x| {
        let v = x as u8;
        let a = v.min(v.wrapping_neg());
        if signed {
            ((v as i8).clamp(-7, 7) + 7) as usize
        } else {
            BOUNDS.partition_point(|&n| n <= a)
        }
    });
    let mut from = 0;
    for (start, packed) in copies.iter().copied().chain([(src.len() as u32, 0)]) {
        let start = start as usize;
        while from < start {
            let y = from / w;
            let x = from % w;
            let len = (w - x).min(start - from);
            if !signed || y.is_multiple_of(2) {
                check_cancelled(cancel)?;
                let row = (y / side) * cols;
                let mut col = x;
                let end = x + len;
                while col < end {
                    let tile = col / side;
                    let end_tile = end.min((tile + 1) * side);
                    let begin = from + col - x;
                    let span = &src[begin..begin + end_tile - col];
                    let (stride, skip) = if signed { (2, col % 2) } else { (1, 0) };
                    let bucket = &mut hist[row + tile];
                    let (sum, n) = &mut avg[row + tile];
                    for &v in span.iter().skip(skip).step_by(stride) {
                        bucket[bins[v as usize]] += 1;
                        *sum += u32::from(v.min(v.wrapping_neg()));
                        *n += 1;
                    }
                    col = end_tile;
                }
            }
            from += len;
        }
        if start < src.len() {
            if !signed || start.is_multiple_of(4) {
                hist[tile_of(start)][16] += 2;
            }
            from = start + usize::from(packed & 4095) + 1;
        }
    }
    let mut order: Vec<_> = avg
        .into_iter()
        .enumerate()
        .map(|(tile, (total, n))| ((u64::from(total) << 16) / u64::from(n.max(1)), tile))
        .collect();
    order.sort_unstable();
    let mut map = vec![0u8; ntiles];
    for (rank, (_, tile)) in order.into_iter().enumerate() {
        map[tile] = (rank * count / ntiles) as u8;
    }
    for _ in 0..rounds {
        let mut tables = [[1u32; BINS]; GROUPS];
        for (&g, h) in map.iter().zip(&hist) {
            for (a, &n) in tables[g as usize].iter_mut().zip(h) {
                *a += 2 * u32::from(n);
            }
        }
        let costs = tables.map(|table| {
            let log = log_cost(table.iter().sum());
            table.map(|n| log - log_cost(n))
        });
        for (g, h) in map.iter_mut().zip(&hist) {
            *g = (0..count)
                .min_by_key(|&g| {
                    costs[g]
                        .iter()
                        .zip(h)
                        .map(|(&a, &b)| a * u32::from(b))
                        .sum::<u32>()
                })
                .unwrap() as u8;
        }
        check_cancelled(cancel)?;
    }
    let mut sizes = [0u32; GROUPS];
    for &g in &map {
        sizes[g as usize] += 1;
    }
    let mut ngroups = 0u8;
    let mut green_hist = [0; GREEN_SYMBOLS];
    let remap = sizes.map(|n| {
        let g = ngroups;
        if n > 0 {
            green_hist[g as usize] = n;
            ngroups += 1;
        }
        g
    });
    for g in &mut map {
        *g = remap[*g as usize];
    }
    Ok(Tiles {
        map,
        ngroups: usize::from(ngroups),
        hist: green_hist,
        side,
    })
}

fn log_cost(n: u32) -> u32 {
    const FRACTION: [u8; 128] = [
        0, 1, 3, 4, 6, 7, 8, 10, 11, 13, 14, 15, 17, 18, 19, 20, 22, 23, 24, 26, 27, 28, 29, 31,
        32, 33, 34, 35, 37, 38, 39, 40, 41, 42, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 55, 56, 57,
        58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80,
        81, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 91, 92, 93, 94, 95, 96, 97, 97, 98, 99,
        100, 101, 102, 103, 103, 104, 105, 106, 107, 107, 108, 109, 110, 111, 111, 112, 113, 114,
        115, 115, 116, 117, 118, 118, 119, 120, 121, 121, 122, 123, 124, 124, 125, 126, 127, 127,
    ];
    let log = n.ilog2();
    let frac = ((n << (31 - log)) >> 24) & 127;
    log * 128 + u32::from(FRACTION[frac as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_across_groups() {
        let (w, h) = (51, 3);
        let mut src = vec![0; w * h];
        let copies: Vec<_> = src
            .chunks_exact_mut(w)
            .enumerate()
            .map(|(y, row)| {
                row[15..33].fill(3);
                // Copy starts in middle tile and ends in the next.
                ((y * w + 16) as u32, 16 | (1 << 12))
            })
            .collect();
        let tiles = classify(&src, w, &|| false).unwrap().unwrap();
        let alpha = codes_for(
            &tiles,
            &src,
            w,
            Bits::bare(Stream::Filtered(1)),
            255,
            &copies,
            &|| false,
        )
        .unwrap()
        .write(&tiles, &src, w, &copies, &|| false)
        .unwrap();
        let rgba: Vec<_> = src.iter().flat_map(|&a| [12, 93, 160, a]).collect();
        let mut opts = crate::EncodeOptions::with_workers(80, 1);
        opts.has_transparency = Some(false);
        let mut frame = crate::encoder::encode(&rgba, w as u32, h as u32, opts, &|| false).unwrap();
        frame.alpha = Some(alpha);
        let decoded = image::load_from_memory(&frame.into_bytes())
            .unwrap()
            .into_rgba8();
        assert!(decoded.as_raw().iter().skip(3).step_by(4).eq(&src));
    }
}
