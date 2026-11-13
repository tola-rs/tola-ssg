//! VP8L alpha streams for masks with long matches in adjacent rows.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::Error;
use crate::encoder::check_cancelled;

use super::{Stream, VP8L_ALPHA, groups};

const MAX_MATCH: usize = 4096;
const MIN_MATCH: usize = 8;
const PROBE_MATCH: usize = 32;
// WebP plane codes for above, left, diagonals, and short vertical periods.
pub(super) const PLANE_CODES: [u32; 8] = [1, 2, 3, 4, 5, 7, 13, 23];
pub(super) const GREEN_SYMBOLS: usize = 280;
pub(super) const DISTANCE_SYMBOLS: usize = 40;
const FIXED_DISTANCE_BITS: usize = 3;
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

pub(super) enum Payload {
    Bytes(Vec<u8>),
    Tiles(Box<TileCandidate>),
}

impl Payload {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Tiles(candidate) => candidate.len,
        }
    }

    pub(super) fn finish(
        self,
        samples: &[u8],
        width: usize,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            Self::Tiles(candidate) => {
                let TileCandidate {
                    tiles,
                    flat,
                    len,
                    stream,
                } = *candidate;
                let grouped = groups::codes_for(
                    &tiles,
                    samples,
                    width,
                    Bits::bare(stream),
                    255,
                    &flat.copies,
                    cancelled,
                )?;
                if grouped.len < len {
                    return grouped.write(&tiles, samples, width, &flat.copies, cancelled);
                }
                flat.write(samples, len, cancelled)
            }
        }
    }
}

pub(super) struct TileCandidate {
    tiles: groups::Tiles,
    flat: Flat,
    len: usize,
    stream: Stream,
}

struct Flat {
    bits: Bits,
    green: [(u32, u32); GREEN_SYMBOLS],
    distance_codes: Option<[(u32, u32); DISTANCE_SYMBOLS]>,
    copies: Vec<(u32, u16)>,
}

pub(super) fn encode(
    samples: &[u8],
    width: usize,
    stream: Stream,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Payload>, Error> {
    if width < 2 || samples.len() < width + PROBE_MATCH || limit < 48 {
        return Ok(None);
    }
    let distances = [
        width,
        1,
        width + 1,
        width - 1,
        2 * width,
        2 * width + 1,
        3 * width,
        4 * width,
    ];
    // Reserve full matching for masks with repeated spans in a bounded probe.
    let stride = (samples.len() / 128).max(1);
    let repeats = (width + 1..samples.len() - PROBE_MATCH + 1)
        .step_by(stride)
        .filter(|&at| {
            distances.iter().any(|&distance| {
                at >= distance
                    && samples[at..at + PROBE_MATCH]
                        == samples[at - distance..at - distance + PROBE_MATCH]
            })
        })
        .take(8)
        .count();
    if repeats < 8 {
        return Ok(None);
    }

    let fixed = fixed_codes(samples);
    let mut bits = Bits::new(stream);
    bits.write(0, 1);
    bits.write(9, 4); // entries through code length nine
    for &length in &CODE_LENGTH_ORDER[..13] {
        bits.write(
            match length {
                6 => 1,
                8 | 9 => 2,
                _ => 0,
            },
            3,
        );
    }
    bits.write(0, 1); // describe the full alphabet
    for &(_, length) in &fixed {
        match length {
            6 => bits.write(0, 1),
            8 => bits.write(1, 2),
            _ => bits.write(3, 2),
        }
    }
    bits.write_unused_channels();
    bits.write_fixed_distance_tree();
    let limit = limit.next_multiple_of(2);
    let mut counts = [0u32; GREEN_SYMBOLS];
    let mut distance_counts = [0u32; DISTANCE_SYMBOLS];
    let mut extra_bits = 0;
    let mut copies = Vec::new();
    let mut at = 0;
    matches(samples, &distances, cancelled, |start, length, distance| {
        count_literals(&samples[at..start], &mut counts, cancelled)?;
        let (symbol, _, count) = copy_token(length, distance);
        counts[symbol] += 1;
        distance_counts[prefix(PLANE_CODES[distance]).0 as usize] += 1;
        extra_bits += count as usize;
        // Length is at most 4096; three high bits select the plane distance.
        copies.push((start as u32, (length as u16 - 1) | (distance as u16) << 12));
        at = start + length;
        Ok(())
    })?;
    count_literals(&samples[at..], &mut counts, cancelled)?;

    let fixed_len = padded_len(bits.len() + extra_bits + symbol_bits(&fixed, &counts));
    let mut adaptive = Bits::new(stream);
    let mut green = fixed;
    let mut selected_len = fixed_len;
    let mut distance_codes = None;
    if let Some(lengths) = code_lengths(&counts, 15) {
        let candidate = codes(&lengths);
        adaptive.write_green_tree(&lengths);
        adaptive.write_unused_channels();
        let green_bits = symbol_bits(&candidate, &counts);
        let mut fixed_distances = adaptive.clone();
        fixed_distances.write_fixed_distance_tree();
        let length_with_fixed = padded_len(fixed_distances.len() + extra_bits + green_bits);
        let codes = adaptive.write_distance_tree(&distance_counts);
        let copy_bits = extra_bits - FIXED_DISTANCE_BITS * copies.len();
        let length_with_codes = padded_len(
            adaptive.len() + copy_bits + green_bits + symbol_bits(&codes, &distance_counts),
        );
        let adaptive_len = length_with_fixed.min(length_with_codes);
        if adaptive_len < fixed_len.min(limit) {
            if length_with_codes < length_with_fixed {
                bits = adaptive;
                distance_codes = Some(codes);
            } else {
                bits = fixed_distances;
            }
            green = candidate;
            selected_len = adaptive_len;
        }
    }

    if selected_len >= limit {
        return Ok(None);
    }
    let flat = Flat {
        bits,
        green,
        distance_codes,
        copies,
    };
    if width >= 64
        && (65_536..=1024 * 1024).contains(&samples.len())
        && selected_len >= samples.len() / 16
        && !flat.copies.is_empty()
        && let Some(tiles) = groups::classify(samples, width, cancelled)?
    {
        return Ok(Some(Payload::Tiles(Box::new(TileCandidate {
            tiles,
            flat,
            len: selected_len,
            stream,
        }))));
    }
    flat.write(samples, selected_len, cancelled)
        .map(Payload::Bytes)
        .map(Some)
}

impl Flat {
    fn write(
        self,
        samples: &[u8],
        len: usize,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        let Self {
            mut bits,
            green,
            distance_codes,
            copies,
        } = self;
        bits.reserve_bytes(len);
        let mut at = 0;
        for (index, &(start, packed)) in copies.iter().enumerate() {
            if index.is_multiple_of(256) {
                check_cancelled(cancelled)?;
            }
            let start = start as usize;
            bits.write_literals(&samples[at..start], &green, cancelled)?;
            let length = usize::from(packed & 4095) + 1;
            let distance = usize::from(packed >> 12);
            if let Some(codes) = &distance_codes {
                let (symbol, count, extra) = prefix(length as u32);
                bits.write_token(green[256 + symbol as usize], extra, count);
                let (symbol, count, extra) = prefix(PLANE_CODES[distance]);
                bits.write_token(codes[symbol as usize], extra, count);
            } else {
                let (symbol, extra, count) = copy_token(length, distance);
                bits.write_token(green[symbol], extra, count);
            }
            at = start + length;
        }
        bits.write_literals(&samples[at..], &green, cancelled)?;
        Ok(bits.finish())
    }
}

fn matches(
    samples: &[u8],
    distances: &[usize; 8],
    cancelled: &(dyn Fn() -> bool + Sync),
    mut matched: impl FnMut(usize, usize, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut at = 0;
    let mut check_at = 0;
    while at < samples.len() {
        if at >= check_at {
            check_cancelled(cancelled)?;
            check_at = at + 4096;
        }
        let max = (samples.len() - at).min(MAX_MATCH);
        if max < MIN_MATCH {
            break;
        }
        let window = &samples[at..at + max];
        let prefix = window.first_chunk::<MIN_MATCH>().unwrap();
        let mut best = (MIN_MATCH - 1, 0);
        for (code, &distance) in distances.iter().enumerate() {
            if distance > at {
                continue;
            }
            let from = at - distance;
            let past = &samples[from..from + max];
            if best.0 < max
                && prefix == past.first_chunk::<MIN_MATCH>().unwrap()
                && (best.0 < MIN_MATCH || window[MIN_MATCH..=best.0] == past[MIN_MATCH..=best.0])
            {
                let start = best.0 + 1;
                best = (
                    start + common_prefix(&window[start..], &past[start..]),
                    code,
                );
            }
        }
        let (length, distance) = best;
        if length >= MIN_MATCH {
            matched(at, length, distance)?;
            at += length;
        } else {
            at += 1;
        }
    }
    Ok(())
}

fn count_literals(
    samples: &[u8],
    counts: &mut [u32; GREEN_SYMBOLS],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    for chunk in samples.chunks(4096) {
        check_cancelled(cancelled)?;
        for &literal in chunk {
            counts[usize::from(literal)] += 1;
        }
    }
    Ok(())
}

fn copy_token(length: usize, distance: usize) -> (usize, u32, u32) {
    let (symbol, length_bits, length_value) = prefix(length as u32);
    let (_, distance_bits, distance_value) = prefix(PLANE_CODES[distance]);
    let distance_code = (distance as u32).reverse_bits() >> 29;
    let extra = length_value | (distance_code | (distance_value << 3)) << length_bits;
    (
        256 + symbol as usize,
        extra,
        length_bits + 3 + distance_bits,
    )
}

pub(super) fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    let (left_blocks, _) = left.as_chunks::<16>();
    let (right_blocks, _) = right.as_chunks::<16>();
    let shared = left_blocks
        .iter()
        .zip(right_blocks)
        .take_while(|(a, b)| a == b)
        .count()
        * 16;
    shared
        + left[shared..]
            .iter()
            .zip(&right[shared..])
            .take_while(|(a, b)| a == b)
            .count()
}

pub(super) fn padded_len(bits: usize) -> usize {
    bits.div_ceil(8).next_multiple_of(2)
}

pub(super) fn symbol_bits<const N: usize>(codes: &[(u32, u32); N], counts: &[u32; N]) -> usize {
    codes
        .iter()
        .zip(counts)
        .map(|(&(_, length), &count)| length as usize * count as usize)
        .sum()
}

/// VP8L's shared length and distance prefix followed by its low bits.
pub(super) fn prefix(value: u32) -> (u32, u32, u32) {
    let value = value - 1;
    if value < 4 {
        return (value, 0, 0);
    }
    let extra = value.ilog2() - 1;
    (
        2 * (extra + 1) + ((value >> extra) & 1),
        extra,
        value & ((1 << extra) - 1),
    )
}

fn fixed_codes(samples: &[u8]) -> [(u32, u32); GREEN_SYMBOLS] {
    let mut counts = [0u32; 256];
    let stride = (samples.len() / 8192).max(1);
    for &sample in samples.iter().step_by(stride) {
        counts[usize::from(sample)] += 1;
    }
    let mut symbols = std::array::from_fn::<_, 256, _>(|index| index);
    symbols.sort_unstable_by_key(|&symbol| (Reverse(counts[symbol]), symbol));
    let mut lengths = [6; GREEN_SYMBOLS];
    lengths[..256].fill(9);
    for &symbol in &symbols[..8] {
        lengths[symbol] = 6;
    }
    for &symbol in &symbols[8..16] {
        lengths[symbol] = 8;
    }
    codes(&lengths)
}

pub(super) fn code_lengths<const N: usize>(counts: &[u32; N], limit: u32) -> Option<[u32; N]> {
    let symbols = counts.iter().filter(|&&count| count != 0).count();
    if symbols < 2 || symbols > 1 << limit {
        return None;
    }
    // Raise small weights only if the unconstrained tree is too deep. When
    // all weights reach the floor, the tree fits for any admissible alphabet.
    let mut floor = 1;
    loop {
        if let Some(lengths) = huffman_lengths(counts, limit, floor) {
            return Some(lengths);
        }
        floor *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::{code_lengths, huffman_lengths};

    #[test]
    fn skewed_histogram_keeps_all_symbols_within_the_limit() {
        let counts = std::array::from_fn::<_, 20, _>(|index| 1 << index);
        assert!(huffman_lengths(&counts, 15, 1).is_none());
        let lengths = code_lengths(&counts, 15).unwrap();
        assert!(lengths.iter().all(|length| (1..=15).contains(length)));
        assert_eq!(
            lengths.iter().map(|length| 1 << (15 - length)).sum::<u32>(),
            1 << 15
        );
    }
}

fn huffman_lengths<const N: usize>(counts: &[u32; N], limit: u32, floor: u64) -> Option<[u32; N]> {
    let mut nodes: BinaryHeap<_> = counts
        .iter()
        .enumerate()
        .filter(|&(_, &count)| count != 0)
        .map(|(index, &count)| Reverse((u64::from(count).max(floor), index)))
        .collect();
    let mut branches = Vec::with_capacity(nodes.len() - 1);
    while nodes.len() > 1 {
        let Reverse((count, left)) = nodes.pop().unwrap();
        let mut right = nodes.peek_mut().unwrap();
        let parent = N + branches.len();
        branches.push((left, right.0.1));
        *right = Reverse((count + right.0.0, parent));
    }

    let mut lengths = [0; N];
    let mut stack = vec![(N + branches.len() - 1, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > limit {
            return None;
        }
        if node < N {
            lengths[node] = depth;
        } else {
            let (left, right) = branches[node - N];
            stack.extend([(left, depth + 1), (right, depth + 1)]);
        }
    }
    Some(lengths)
}

pub(super) fn codes<const N: usize>(lengths: &[u32; N]) -> [(u32, u32); N] {
    let mut counts = [0; 16];
    for &length in lengths {
        counts[length as usize] += u32::from(length != 0);
    }
    let mut next = [0u32; 16];
    for length in 1..next.len() {
        next[length] = (next[length - 1] + counts[length - 1]) << 1;
    }
    lengths.map(|length| {
        if length == 0 {
            return (0, 0);
        }
        let code = &mut next[length as usize];
        let reversed = code.reverse_bits() >> (32 - length);
        *code += 1;
        (reversed, length)
    })
}

#[derive(Clone)]
pub(super) struct Bits {
    bytes: Vec<u8>,
    pending: u64,
    count: u32,
}

impl Bits {
    pub(super) fn new(stream: Stream) -> Self {
        let mut bits = Self::bare(stream);
        bits.write(0, 2); // no colour cache or meta Huffman groups
        bits
    }

    pub(super) fn header(method: u8) -> Self {
        Self {
            bytes: vec![method],
            pending: 0,
            count: 0,
        }
    }

    pub(super) fn bare(stream: Stream) -> Self {
        let mut bits = Self::header(match stream {
            Stream::Filtered(method) => method,
            Stream::HalfGradient => VP8L_ALPHA,
        });
        if matches!(stream, Stream::HalfGradient) {
            bits.write(1, 3); // predictor transform
            bits.write(3, 3); // 32×32 predictor tiles
            bits.write(0, 1); // no colour cache in the predictor map
            for value in [13, 0, 0, 0, 0] {
                bits.write_single_tree(value);
            }
        }
        bits.write(0, 1); // end of transforms
        bits
    }

    pub(super) fn palette(method: u8, palette: &[u8]) -> Self {
        let mut bits = Self {
            bytes: vec![method],
            pending: 0,
            count: 0,
        };
        bits.write(7, 3);
        bits.write((palette.len() - 1) as u32, 8);
        bits.write(0, 1);
        let mut deltas = [0; 16];
        let mut counts = [0; GREEN_SYMBOLS];
        let mut previous = 0;
        for (&value, delta) in palette.iter().zip(&mut deltas) {
            *delta = value.wrapping_sub(previous);
            previous = value;
            counts[*delta as usize] += 1;
        }
        let tree = if let Some(lengths) = code_lengths(&counts, 15) {
            bits.write_green_tree(&lengths);
            Some(codes(&lengths))
        } else {
            bits.write_single_tree(u32::from(deltas[0]));
            None
        };
        for value in [0, 0, 255, 0] {
            bits.write_single_tree(value);
        }
        if let Some(tree) = tree {
            for &symbol in &deltas[..palette.len()] {
                let (code, len) = tree[symbol as usize];
                bits.write(code, len);
            }
        }
        bits.write(0, 3);
        bits
    }
    pub(super) fn len(&self) -> usize {
        self.bytes.len() * 8 + self.count as usize
    }

    pub(super) fn reserve_bytes(&mut self, total: usize) {
        self.bytes.reserve_exact(total - self.bytes.len());
    }

    pub(super) fn finish(mut self) -> Vec<u8> {
        let tail = self.pending.to_le_bytes();
        self.bytes
            .extend_from_slice(&tail[..self.count.div_ceil(8) as usize]);
        self.bytes
    }

    pub(super) fn write_token(&mut self, (code, length): (u32, u32), extra: u32, count: u32) {
        self.write(code, length);
        self.write(extra, count);
    }

    pub(super) fn write_literals(
        &mut self,
        samples: &[u8],
        green: &[(u32, u32); GREEN_SYMBOLS],
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), Error> {
        for chunk in samples.chunks(4096) {
            check_cancelled(cancelled)?;
            for &sample in chunk {
                let (code, count) = green[usize::from(sample)];
                self.write(code, count);
            }
        }
        Ok(())
    }

    pub(super) fn write_green_tree(&mut self, lengths: &[u32; GREEN_SYMBOLS]) {
        self.write_tree(lengths);
    }

    pub(super) fn write_single_tree(&mut self, symbol: u32) {
        self.write(1, 2);
        self.write(u32::from(symbol > 1), 1);
        self.write(symbol, if symbol > 1 { 8 } else { 1 });
    }

    pub(super) fn write_tree(&mut self, lengths: &[u32]) {
        let mut counts = [0; 19];
        for &length in lengths {
            counts[length as usize] += 1;
        }
        let plain = LengthTree::new(&counts);
        let mut compact_counts = [0; 19];
        let mut extra_bits = 0;
        visit_length_runs(lengths, |symbol, _, width| {
            compact_counts[symbol] += 1;
            extra_bits += width as usize;
        });
        if compact_counts != counts {
            let compact = LengthTree::new(&compact_counts);
            if compact.bits(&compact_counts) + extra_bits < plain.bits(&counts) {
                compact.write(self);
                visit_length_runs(lengths, |symbol, extra, width| {
                    self.write_token(compact.codes[symbol], extra, width);
                });
                return;
            }
        }
        plain.write(self);
        for &length in lengths {
            let (code, width) = plain.codes[length as usize];
            self.write(code, width);
        }
    }

    pub(super) fn write_distance_tree(
        &mut self,
        counts: &[u32; DISTANCE_SYMBOLS],
    ) -> [(u32, u32); DISTANCE_SYMBOLS] {
        if let Some(lengths) = code_lengths(counts, 15) {
            self.write_tree(&lengths);
            codes(&lengths)
        } else {
            let symbol = counts.iter().position(|&count| count != 0).unwrap_or(0);
            self.write_single_tree(symbol as u32);
            [(0, 0); DISTANCE_SYMBOLS]
        }
    }

    pub(super) fn write_unused_channels(&mut self) {
        for symbol in [0, 0, 255] {
            self.write_single_tree(symbol);
        }
    }

    fn write_fixed_distance_tree(&mut self) {
        self.write(0, 1);
        self.write(2, 4); // first six entries include code lengths zero and three
        for length in [0, 0, 1, 0, 0, 1] {
            self.write(length, 3);
        }
        self.write(0, 1);
        for symbol in 0..DISTANCE_SYMBOLS as u32 {
            self.write(
                u32::from(PLANE_CODES.iter().any(|&code| prefix(code).0 == symbol)),
                1,
            );
        }
    }

    pub(super) fn write(&mut self, value: u32, count: u32) {
        self.pending |= u64::from(value) << self.count;
        self.count += count;
        if self.count >= 32 {
            let word = self.pending as u32;
            self.bytes.extend_from_slice(&word.to_le_bytes());
            self.pending >>= 32;
            self.count -= 32;
        }
    }
}

struct LengthTree {
    lengths: [u32; 19],
    codes: [(u32, u32); 19],
    entries: usize,
}

impl LengthTree {
    fn new(counts: &[u32; 19]) -> Self {
        let lengths =
            code_lengths(counts, 7).unwrap_or_else(|| counts.map(|count| u32::from(count != 0)));
        let entries = (1 + CODE_LENGTH_ORDER
            .iter()
            .rposition(|&index| lengths[index] != 0)
            .unwrap())
        .max(4);
        let codes = if counts.iter().filter(|&&count| count != 0).count() == 1 {
            [(0, 0); 19]
        } else {
            codes(&lengths)
        };
        Self {
            lengths,
            codes,
            entries,
        }
    }

    fn bits(&self, counts: &[u32; 19]) -> usize {
        6 + self.entries * 3 + symbol_bits(&self.codes, counts)
    }

    fn write(&self, bits: &mut Bits) {
        bits.write(0, 1);
        bits.write((self.entries - 4) as u32, 4);
        for &index in &CODE_LENGTH_ORDER[..self.entries] {
            bits.write(self.lengths[index], 3);
        }
        bits.write(0, 1); // describe the full alphabet
    }
}

fn visit_length_runs(mut lengths: &[u32], mut write: impl FnMut(usize, u32, u32)) {
    while let Some(&length) = lengths.first() {
        if length == 0 {
            let zeros = lengths.iter().take(138).take_while(|&&n| n == 0).count();
            if zeros >= 11 {
                write(18, zeros as u32 - 11, 7);
                lengths = &lengths[zeros..];
                continue;
            }
            if zeros >= 3 {
                write(17, zeros as u32 - 3, 3);
                lengths = &lengths[zeros..];
                continue;
            }
        }
        write(length as usize, 0, 0);
        lengths = &lengths[1..];
    }
}
