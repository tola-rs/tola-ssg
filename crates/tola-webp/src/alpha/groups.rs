//! VP8L entropy groups for alpha residuals with different local distributions.

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
    groups: Vec<u8>,
    group_count: usize,
    counts: [u32; GREEN_SYMBOLS],
    side: usize,
}

impl Tiles {
    pub(super) fn single(samples: usize, width: usize) -> Self {
        let count = width.div_ceil(TILE) * (samples / width).div_ceil(TILE);
        let mut counts = [0; GREEN_SYMBOLS];
        counts[0] = count as u32;
        Self {
            groups: vec![0; count],
            group_count: 1,
            counts,
            side: TILE,
        }
    }
}

pub(super) fn classify(
    samples: &[u8],
    width: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Tiles>, Error> {
    let height = samples.len() / width;
    let columns = width.div_ceil(TILE);
    let rows = height.div_ceil(TILE);
    let mut groups = vec![0u8; columns * rows];
    let mut sizes = [0usize; 3];
    for (ty, tiles) in groups.chunks_exact_mut(columns).enumerate() {
        check_cancelled(cancelled)?;
        for (tx, group) in tiles.iter_mut().enumerate() {
            let y = ty * TILE;
            let x = tx * TILE;
            let mut total = 0;
            let mut irregular = 0;
            for y in (y..(y + TILE).min(height)).step_by(4) {
                let row = &samples[y * width..][..width];
                for &value in row[x..(x + TILE).min(width)].iter().step_by(4) {
                    total += 1;
                    irregular += usize::from(value.min(value.wrapping_neg()) > 2);
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
    if ranked[1] * 32 < groups.len() {
        return Ok(None);
    }
    let mut count = 0;
    let map = sizes.map(|size| {
        let group = count;
        count += u8::from(size != 0);
        group
    });
    let mut counts = [0; GREEN_SYMBOLS];
    for group in &mut groups {
        *group = map[*group as usize];
        counts[*group as usize] += 1;
    }
    Ok(Some(Tiles {
        groups,
        group_count: count as usize,
        counts,
        side: TILE,
    }))
}

pub(super) struct Codes {
    bits: Bits,
    green: [[(u32, u32); GREEN_SYMBOLS]; GROUPS],
    distance: [[(u32, u32); DISTANCE_SYMBOLS]; GROUPS],
    pub(super) len: usize,
}

pub(super) fn codes_for(
    tiles: &Tiles,
    samples: &[u8],
    width: usize,
    mut bits: Bits,
    alpha: u32,
    copies: &[(u32, u16)],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Codes, Error> {
    let Tiles {
        groups,
        group_count,
        counts,
        side,
    } = tiles;
    let mut green = [[0; GREEN_SYMBOLS]; GROUPS];
    let mut dist = [[0; DISTANCE_SYMBOLS]; GROUPS];
    let mut extra = 0;
    visit(
        samples,
        width,
        copies,
        groups,
        *side,
        cancelled,
        |group, token| {
            match token {
                Token::Literals(values) => {
                    for &v in values {
                        green[group][v as usize] += 1;
                    }
                }
                Token::Copy(packed) => {
                    let (length, distance) = copy_prefixes(packed);
                    green[group][256 + length.0 as usize] += 1;
                    dist[group][distance.0 as usize] += 1;
                    extra += (length.1 + distance.1) as usize;
                }
            }
            Ok(())
        },
    )?;
    bits.write(if *group_count == 1 { 0 } else { 2 }, 2);
    if *group_count != 1 {
        bits.write(side.ilog2() - 2, 3);
        bits.write(0, 1); // no colour cache in the entropy image
        let map_codes = bits.write_green_codes(counts);
        for _ in 0..4 {
            bits.write_single_tree(0);
        }
        bits.write_literals(groups, &map_codes, cancelled)?;
    }
    let mut green_codes = [[(0, 0); GREEN_SYMBOLS]; GROUPS];
    let mut distance_codes = [[(0, 0); DISTANCE_SYMBOLS]; GROUPS];
    for g in 0..*group_count {
        green_codes[g] = bits.write_green_codes(&green[g]);
        for value in [0, 0, alpha] {
            bits.write_single_tree(value);
        }
        distance_codes[g] = bits.write_distance_tree(&dist[g]);
        extra +=
            symbol_bits(&green_codes[g], &green[g]) + symbol_bits(&distance_codes[g], &dist[g]);
    }
    let len = padded_len(bits.len() + extra);
    Ok(Codes {
        bits,
        green: green_codes,
        distance: distance_codes,
        len,
    })
}

impl Codes {
    pub(super) fn write(
        self,
        tiles: &Tiles,
        samples: &[u8],
        width: usize,
        copies: &[(u32, u16)],
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        let Self {
            mut bits,
            green: green_codes,
            distance: distance_codes,
            len,
        } = self;
        bits.reserve_bytes(len);
        visit(
            samples,
            width,
            copies,
            &tiles.groups,
            tiles.side,
            cancelled,
            |group, token| {
                match token {
                    Token::Literals(values) => {
                        for &value in values {
                            let (code, len) = green_codes[group][value as usize];
                            bits.write(code, len);
                        }
                    }
                    Token::Copy(packed) => {
                        let (length, distance) = copy_prefixes(packed);
                        bits.write_token(
                            green_codes[group][256 + length.0 as usize],
                            length.2,
                            length.1,
                        );
                        bits.write_token(
                            distance_codes[group][distance.0 as usize],
                            distance.2,
                            distance.1,
                        );
                    }
                }
                Ok(())
            },
        )?;
        Ok(bits.finish())
    }
}

impl Bits {
    pub(super) fn write_green_codes(
        &mut self,
        counts: &[u32; GREEN_SYMBOLS],
    ) -> [(u32, u32); GREEN_SYMBOLS] {
        if let Some(lengths) = code_lengths(counts, 15) {
            self.write_green_tree(&lengths);
            codes(&lengths)
        } else {
            let symbol = counts.iter().position(|&n| n != 0).unwrap_or(0);
            if symbol < 256 {
                self.write_single_tree(symbol as u32);
            } else {
                let mut lengths = [0; GREEN_SYMBOLS];
                lengths[symbol] = 1;
                self.write_green_tree(&lengths);
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
    samples: &[u8],
    width: usize,
    mut copies: &[(u32, u16)],
    groups: &[u8],
    side: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
    mut write: impl FnMut(usize, Token<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    let columns = width.div_ceil(side);
    let mut at = 0;
    let mut check_at = 0;
    while at < samples.len() {
        if at >= check_at {
            check_cancelled(cancelled)?;
            check_at = at + 4096;
        }
        let y = at / width;
        let x = at % width;
        let group = groups[(y / side) * columns + x / side] as usize;
        if let Some((&copy, rest)) = copies.split_first()
            && at == copy.0 as usize
        {
            // A spanning copy uses the group at its starting pixel.
            write(group, Token::Copy(copy.1))?;
            at += usize::from(copy.1 & 4095) + 1;
            copies = rest;
            continue;
        }
        let end = at + (side - x % side).min(width - x);
        let end = copies
            .first()
            .map_or(end, |&(start, _)| end.min(start as usize));
        write(group, Token::Literals(&samples[at..end]))?;
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
    samples: &[u8],
    width: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Tiles, Error> {
    let height = samples.len() / width;
    let mut groups = Vec::with_capacity(width.div_ceil(TILE) * height.div_ceil(TILE));
    let mut sizes = [0; 5];
    for y in (0..height).step_by(TILE) {
        check_cancelled(cancelled)?;
        for x in (0..width).step_by(TILE) {
            let mut total = 0;
            let mut count = 0;
            for y in (y..(y + TILE).min(height)).step_by(2) {
                for &v in samples[y * width + x..y * width + (x + TILE).min(width)]
                    .iter()
                    .step_by(2)
                {
                    let value = u32::from(v.min(v.wrapping_neg()));
                    total += value * value;
                    count += 1;
                }
            }
            let group = [2, 10, 30, 100].partition_point(|&bound| total > bound * count);
            groups.push(group as u8);
            sizes[group] += 1;
        }
    }
    let mut count = 0u8;
    let mut counts = [0; GREEN_SYMBOLS];
    let map = sizes.map(|n| {
        let group = count;
        if n != 0 {
            counts[group as usize] = n;
            count += 1;
        }
        group
    });
    for g in &mut groups {
        *g = map[*g as usize];
    }
    Ok(Tiles {
        groups,
        group_count: usize::from(count),
        counts,
        side: TILE,
    })
}

pub(super) fn clustered(
    samples: &[u8],
    width: usize,
    copies: &[(u32, u16)],
    side: usize,
    count: usize,
    rounds: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Tiles, Error> {
    const BINS: usize = 17;
    const BOUNDS: [u8; 15] = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96];
    let columns = width.div_ceil(side);
    let tile_count = columns * (samples.len() / width).div_ceil(side);
    let mut hist = vec![[0u16; BINS]; tile_count];
    let mut average = vec![(0u32, 0u32); tile_count];
    let tile = |at: usize| (at / width / side) * columns + (at % width / side);
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
    for (start, packed) in copies.iter().copied().chain([(samples.len() as u32, 0)]) {
        let start = start as usize;
        while from < start {
            let y = from / width;
            let x = from % width;
            let length = (width - x).min(start - from);
            if !signed || y.is_multiple_of(2) {
                check_cancelled(cancelled)?;
                let row = (y / side) * columns;
                let mut column = x;
                let end = x + length;
                while column < end {
                    let tile = column / side;
                    let end_of_tile = end.min((tile + 1) * side);
                    let begin = from + column - x;
                    let span = &samples[begin..begin + end_of_tile - column];
                    let (stride, skip) = if signed { (2, column % 2) } else { (1, 0) };
                    let bucket = &mut hist[row + tile];
                    let (sum, count) = &mut average[row + tile];
                    for &value in span.iter().skip(skip).step_by(stride) {
                        bucket[bins[value as usize]] += 1;
                        *sum += u32::from(value.min(value.wrapping_neg()));
                        *count += 1;
                    }
                    column = end_of_tile;
                }
            }
            from += length;
        }
        if start < samples.len() {
            if !signed || start.is_multiple_of(4) {
                hist[tile(start)][16] += 2;
            }
            from = start + usize::from(packed & 4095) + 1;
        }
    }
    let mut order: Vec<_> = average
        .into_iter()
        .enumerate()
        .map(|(tile, (total, n))| ((u64::from(total) << 16) / u64::from(n.max(1)), tile))
        .collect();
    order.sort_unstable();
    let mut groups = vec![0u8; tile_count];
    for (rank, (_, tile)) in order.into_iter().enumerate() {
        groups[tile] = (rank * count / tile_count) as u8;
    }
    for _ in 0..rounds {
        let mut tables = [[1u32; BINS]; GROUPS];
        for (&g, h) in groups.iter().zip(&hist) {
            for (a, &n) in tables[g as usize].iter_mut().zip(h) {
                *a += 2 * u32::from(n);
            }
        }
        let costs = tables.map(|table| {
            let log = log_cost(table.iter().sum());
            table.map(|n| log - log_cost(n))
        });
        for (group, h) in groups.iter_mut().zip(&hist) {
            *group = (0..count)
                .min_by_key(|&g| {
                    costs[g]
                        .iter()
                        .zip(h)
                        .map(|(&a, &b)| a * u32::from(b))
                        .sum::<u32>()
                })
                .unwrap() as u8;
        }
        check_cancelled(cancelled)?;
    }
    let mut sizes = [0u32; GROUPS];
    for &g in &groups {
        sizes[g as usize] += 1;
    }
    let mut group_count = 0u8;
    let mut counts = [0; GREEN_SYMBOLS];
    let map = sizes.map(|n| {
        let g = group_count;
        if n > 0 {
            counts[g as usize] = n;
            group_count += 1;
        }
        g
    });
    for g in &mut groups {
        *g = map[*g as usize];
    }
    Ok(Tiles {
        groups,
        group_count: usize::from(group_count),
        counts,
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
    let fraction = ((n << (31 - log)) >> 24) & 127;
    log * 128 + u32::from(FRACTION[fraction as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_alpha_decodes_a_copy_across_entropy_groups() {
        let (width, height) = (51, 3);
        let mut samples = vec![0; width * height];
        let copies: Vec<_> = samples
            .chunks_exact_mut(width)
            .enumerate()
            .map(|(y, row)| {
                row[15..33].fill(3);
                // This copy begins in the middle tile and ends in the next.
                // Its length is the only green symbol in its entropy group.
                ((y * width + 16) as u32, 16 | (1 << 12))
            })
            .collect();
        let tiles = classify(&samples, width, &|| false).unwrap().unwrap();
        let alpha = codes_for(
            &tiles,
            &samples,
            width,
            Bits::bare(Stream::Filtered(1)),
            255,
            &copies,
            &|| false,
        )
        .unwrap()
        .write(&tiles, &samples, width, &copies, &|| false)
        .unwrap();
        let rgba: Vec<_> = samples
            .iter()
            .flat_map(|&alpha| [12, 93, 160, alpha])
            .collect();
        let mut options = crate::EncodeOptions::with_workers(80, 1);
        options.has_transparency = Some(false);
        let mut frame =
            crate::encoder::encode(&rgba, width as u32, height as u32, options, &|| false).unwrap();
        frame.alpha = Some(alpha);
        let decoded = image::load_from_memory(&frame.into_bytes())
            .unwrap()
            .into_rgba8();
        assert!(decoded.as_raw().iter().skip(3).step_by(4).eq(&samples));
    }
}
