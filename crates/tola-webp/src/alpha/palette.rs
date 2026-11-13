//! VP8L colour indexing for alpha residuals with at most sixteen values.
use super::repeated::{
    Bits, GREEN_SYMBOLS, code_lengths, codes, common_prefix, prefix, symbol_bits,
};
use crate::Error;
use crate::encoder::check_cancelled;

const TABLE: usize = 1 << 15;
const WINDOW: usize = (1 << 20) - 120;
const MAX_MATCH: usize = 4096;
const MIN_MATCH: usize = 4;
const NEAR_CODES: [u32; 15] = [1, 2, 3, 4, 5, 7, 13, 23, 8, 9, 10, 11, 12, 15, 16];

pub(super) fn encode(
    samples: &mut [u8],
    width: usize,
    height: usize,
    method: u8,
    limit: usize,
    extended: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    if width < 8 || !(64 * 1024..=1024 * 1024).contains(&samples.len()) || limit < 4096 {
        return Ok(None);
    }

    // Four-bit indices can pack at least two samples into each byte.
    let mut seen = [false; 256];
    let mut count = 0;
    for chunk in samples.chunks(4096) {
        check_cancelled(cancelled)?;
        for &v in chunk {
            if !seen[v as usize] {
                seen[v as usize] = true;
                count += 1;
                if count > 16 {
                    return Ok(None);
                }
            }
        }
    }
    if count < 2 {
        return Ok(None);
    }
    let mut palette = Vec::with_capacity(count);
    let mut lookup = [0; 256];
    for (value, found) in seen.into_iter().enumerate() {
        if found {
            lookup[value] = palette.len() as u8;
            palette.push(value as u8);
        }
    }
    let (per, bits) = if count <= 2 {
        (8, 1)
    } else if count <= 4 {
        (4, 2)
    } else {
        (2, 4)
    };
    let packed_width = width.div_ceil(per);
    // Packed indices occupy fewer bytes than the residuals. Read each group before writing it.
    for y in 0..height {
        check_cancelled(cancelled)?;
        for x in 0..packed_width {
            let start = y * width + x * per;
            let end = (start + per).min((y + 1) * width);
            let index = samples[start..end]
                .iter()
                .enumerate()
                .fold(0, |word, (index, &value)| {
                    word | lookup[value as usize] << (index * bits)
                });
            samples[y * packed_width + x] = index;
        }
    }
    let width = packed_width;
    let samples = &samples[..width * height];
    let mut green_counts = [0; GREEN_SYMBOLS];
    let mut distance_counts = [0; 40];
    let mut extras = 0;
    let mut copies = Vec::new();
    let near = [
        width,
        1,
        width + 1,
        width - 1,
        2 * width,
        2 * width + 1,
        3 * width,
        4 * width,
        2 * width - 1,
        width + 2,
        width - 2,
        2 * width + 2,
        2 * width - 2,
        3 * width + 1,
        3 * width - 1,
    ];
    let mut table = vec![u32::MAX; TABLE];
    let mut at = 0;
    let mut check_at = 0;
    while at + MIN_MATCH <= samples.len() {
        if at >= check_at {
            check_cancelled(cancelled)?;
            check_at = at + 4096;
        }
        let slot = hash(samples, at);
        let old = std::mem::replace(&mut table[slot], at as u32) as usize;
        let max = (samples.len() - at).min(MAX_MATCH);
        let mut length = MIN_MATCH - 1;
        let mut plane = 0;
        for (distance, code) in near
            .into_iter()
            .zip(NEAR_CODES)
            .take(if extended { 15 } else { 8 })
            .chain((old < at && at - old <= WINDOW).then(|| {
                let distance = at - old;
                (distance, distance as u32 + 120)
            }))
        {
            if distance <= at
                && length < max
                && samples[at..at + length + 1]
                    == samples[at - distance..at - distance + length + 1]
            {
                let start = at + length + 1;
                length += 1 + common_prefix(
                    &samples[start..at + max],
                    &samples[start - distance..at + max - distance],
                );
                plane = code;
            }
        }
        if plane == 0 {
            green_counts[usize::from(samples[at])] += 1;
            at += 1;
            continue;
        }
        let (len, len_bits, _) = prefix(length as u32);
        let (dist, dist_bits, _) = prefix(plane);
        green_counts[256 + len as usize] += 1;
        distance_counts[dist as usize] += 1;
        extras += (len_bits + dist_bits) as usize;
        copies.push((at as u32, plane << 12 | (length as u32 - 1)));
        // Future matches can begin inside a copied span. Sample densely near its end.
        for index in (at + 1..at + length)
            .step_by(4)
            .chain(at + length - 3..at + length)
        {
            if index + MIN_MATCH <= samples.len() {
                table[hash(samples, index)] = index as u32;
            }
        }
        at += length;
    }
    drop(table);
    for &sample in &samples[at..] {
        green_counts[usize::from(sample)] += 1;
    }
    let Some(green_lengths) = code_lengths(&green_counts, 15) else {
        return Ok(None);
    };
    let mut bits = Bits::palette(method, &palette);
    bits.write_green_tree(&green_lengths);
    let green = codes(&green_lengths);
    for value in [0, 0, 0] {
        bits.write_single_tree(value);
    }
    let distance = bits.write_distance_tree(&distance_counts);
    let size = bits.len()
        + extras
        + symbol_bits(&green, &green_counts)
        + symbol_bits(&distance, &distance_counts);
    let size = size.div_ceil(8);
    if size.next_multiple_of(2) >= limit.next_multiple_of(2) {
        return Ok(None);
    }
    bits.reserve_bytes(size);
    let mut at = 0;
    for (index, (start, packed)) in copies.into_iter().enumerate() {
        if index.is_multiple_of(256) {
            check_cancelled(cancelled)?;
        }
        let start = start as usize;
        bits.write_literals(&samples[at..start], &green, cancelled)?;
        let length = (packed & 4095) + 1;
        let (len, extra, value) = prefix(length);
        let (code, count) = green[256 + len as usize];
        bits.write(code, count);
        bits.write(value, extra);
        let (dist, extra, value) = prefix(packed >> 12);
        let (code, count) = distance[dist as usize];
        bits.write(code, count);
        bits.write(value, extra);
        at = start + length as usize;
    }
    bits.write_literals(&samples[at..], &green, cancelled)?;
    Ok(Some(bits.finish()))
}

fn hash(samples: &[u8], at: usize) -> usize {
    let value = u32::from_le_bytes(samples[at..at + 4].try_into().unwrap());
    (value.wrapping_mul(0x9e37_79b1) >> (u32::BITS - TABLE.ilog2())) as usize
}
