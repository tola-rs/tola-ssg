//! VP8L alpha streams for masks with repeated spans across adjacent rows.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::Error;
use crate::encoder::check_cancelled;

use super::{Stream, VP8L_ALPHA, groups};

const MAX_MATCH: usize = 4096;
const MIN_MATCH: usize = 8;
const PROBE_MATCH: usize = 32;
pub(super) const PLANE_CODES: [u32; 8] = [1, 2, 3, 4, 5, 7, 13, 23];
pub(super) const GREEN_SYMBOLS: usize = 280;
pub(super) const DISTANCE_SYMBOLS: usize = 40;
const FIXED_DISTANCE_BITS: usize = 3;
const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

pub(super) enum AlphaBlock {
    Bytes(Vec<u8>),
    Tiles(Box<TileCand>),
}

impl AlphaBlock {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Bytes(buf) => buf.len(),
            Self::Tiles(cand) => cand.len,
        }
    }

    pub(super) fn finish(
        self,
        src: &[u8],
        w: usize,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        match self {
            Self::Bytes(buf) => Ok(buf),
            Self::Tiles(cand) => {
                let TileCand {
                    tiles,
                    flat,
                    len,
                    stream,
                } = *cand;
                let grouped = groups::codes_for(
                    &tiles,
                    src,
                    w,
                    Bits::bare(stream),
                    255,
                    &flat.copies,
                    cancel,
                )?;
                if grouped.len < len {
                    return grouped.write(&tiles, src, w, &flat.copies, cancel);
                }
                flat.write(src, len, cancel)
            }
        }
    }
}

pub(super) struct TileCand {
    tiles: groups::Tiles,
    flat: Flat,
    len: usize,
    stream: Stream,
}

struct Flat {
    bits: Bits,
    green: [(u32, u32); GREEN_SYMBOLS],
    dist_codes: Option<[(u32, u32); DISTANCE_SYMBOLS]>,
    copies: Vec<(u32, u16)>,
}

pub(super) fn encode(
    src: &[u8],
    w: usize,
    stream: Stream,
    limit: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<AlphaBlock>, Error> {
    if w < 2 || src.len() < w + PROBE_MATCH || limit < 48 {
        return Ok(None);
    }
    let dists = [w, 1, w + 1, w - 1, 2 * w, 2 * w + 1, 3 * w, 4 * w];
    let stride = (src.len() / 128).max(1);
    let repeats = (w + 1..src.len() - PROBE_MATCH + 1)
        .step_by(stride)
        .filter(|&at| {
            dists
                .iter()
                .any(|&d| at >= d && src[at..at + PROBE_MATCH] == src[at - d..at - d + PROBE_MATCH])
        })
        .take(8)
        .count();
    if repeats < 8 {
        return Ok(None);
    }

    let fixed = fixed_codes(src);
    let mut bits = Bits::new(stream);
    bits.write(0, 1);
    bits.write(9, 4);
    for &len in &CODE_LENGTH_ORDER[..13] {
        bits.write(
            match len {
                6 => 1,
                8 | 9 => 2,
                _ => 0,
            },
            3,
        );
    }
    bits.write(0, 1);
    for &(_, len) in &fixed {
        match len {
            6 => bits.write(0, 1),
            8 => bits.write(1, 2),
            _ => bits.write(3, 2),
        }
    }
    bits.write_unused_channels();
    bits.write_fixed_distance_tree();
    let limit = limit.next_multiple_of(2);
    let mut hist = [0u32; GREEN_SYMBOLS];
    let mut dist_hist = [0u32; DISTANCE_SYMBOLS];
    let mut extra = 0;
    let mut copies = Vec::new();
    let mut at = 0;
    matches(src, &dists, cancel, |start, len, dist| {
        count_literals(&src[at..start], &mut hist, cancel)?;
        let (sym, _, nbits) = copy_token(len, dist);
        hist[sym] += 1;
        dist_hist[prefix(PLANE_CODES[dist]).0 as usize] += 1;
        extra += nbits as usize;
        // Copy length <= 4096; top 3 bits encode the plane distance index.
        copies.push((start as u32, (len as u16 - 1) | (dist as u16) << 12));
        at = start + len;
        Ok(())
    })?;
    count_literals(&src[at..], &mut hist, cancel)?;

    let fixed_len = padded_len(bits.len() + extra + symbol_bits(&fixed, &hist));
    let mut adaptive = Bits::new(stream);
    let mut green = fixed;
    let mut selected_len = fixed_len;
    let mut dist_codes = None;
    if let Some(lens) = code_lengths(&hist, 15) {
        let cand = codes(&lens);
        adaptive.write_green_tree(&lens);
        adaptive.write_unused_channels();
        let green_bits = symbol_bits(&cand, &hist);
        let mut fixed_dist = adaptive.clone();
        fixed_dist.write_fixed_distance_tree();
        let len_fixed = padded_len(fixed_dist.len() + extra + green_bits);
        let dcodes = adaptive.write_distance_tree(&dist_hist);
        let copy_bits = extra - FIXED_DISTANCE_BITS * copies.len();
        let len_codes =
            padded_len(adaptive.len() + copy_bits + green_bits + symbol_bits(&dcodes, &dist_hist));
        let adaptive_len = len_fixed.min(len_codes);
        if adaptive_len < fixed_len.min(limit) {
            if len_codes < len_fixed {
                bits = adaptive;
                dist_codes = Some(dcodes);
            } else {
                bits = fixed_dist;
            }
            green = cand;
            selected_len = adaptive_len;
        }
    }

    if selected_len >= limit {
        return Ok(None);
    }
    let flat = Flat {
        bits,
        green,
        dist_codes,
        copies,
    };
    if w >= 64
        && (65_536..=1024 * 1024).contains(&src.len())
        && selected_len >= src.len() / 16
        && !flat.copies.is_empty()
        && let Some(tiles) = groups::classify(src, w, cancel)?
    {
        return Ok(Some(AlphaBlock::Tiles(Box::new(TileCand {
            tiles,
            flat,
            len: selected_len,
            stream,
        }))));
    }
    flat.write(src, selected_len, cancel)
        .map(AlphaBlock::Bytes)
        .map(Some)
}

impl Flat {
    fn write(
        self,
        src: &[u8],
        len: usize,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, Error> {
        let Self {
            mut bits,
            green,
            dist_codes,
            copies,
        } = self;
        bits.reserve_bytes(len);
        let mut at = 0;
        for (i, &(start, packed)) in copies.iter().enumerate() {
            if i.is_multiple_of(256) {
                check_cancelled(cancel)?;
            }
            let start = start as usize;
            bits.write_literals(&src[at..start], &green, cancel)?;
            let copy_len = usize::from(packed & 4095) + 1;
            let dist = usize::from(packed >> 12);
            if let Some(dcodes) = &dist_codes {
                let (sym, nbits, extra) = prefix(copy_len as u32);
                bits.write_token(green[256 + sym as usize], extra, nbits);
                let (sym, nbits, extra) = prefix(PLANE_CODES[dist]);
                bits.write_token(dcodes[sym as usize], extra, nbits);
            } else {
                let (sym, extra, nbits) = copy_token(copy_len, dist);
                bits.write_token(green[sym], extra, nbits);
            }
            at = start + copy_len;
        }
        bits.write_literals(&src[at..], &green, cancel)?;
        Ok(bits.finish())
    }
}

fn matches(
    src: &[u8],
    dists: &[usize; 8],
    cancel: &(dyn Fn() -> bool + Sync),
    mut matched: impl FnMut(usize, usize, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut at = 0;
    let mut check_at = 0;
    while at < src.len() {
        if at >= check_at {
            check_cancelled(cancel)?;
            check_at = at + 4096;
        }
        let max = (src.len() - at).min(MAX_MATCH);
        if max < MIN_MATCH {
            break;
        }
        let win = &src[at..at + max];
        let pfx = win.first_chunk::<MIN_MATCH>().unwrap();
        let mut best = (MIN_MATCH - 1, 0);
        for (code, &dist) in dists.iter().enumerate() {
            if dist > at {
                continue;
            }
            let from = at - dist;
            let past = &src[from..from + max];
            if best.0 < max
                && pfx == past.first_chunk::<MIN_MATCH>().unwrap()
                && (best.0 < MIN_MATCH || win[MIN_MATCH..=best.0] == past[MIN_MATCH..=best.0])
            {
                let start = best.0 + 1;
                best = (start + common_prefix(&win[start..], &past[start..]), code);
            }
        }
        let (len, dist) = best;
        if len >= MIN_MATCH {
            matched(at, len, dist)?;
            at += len;
        } else {
            at += 1;
        }
    }
    Ok(())
}

fn count_literals(
    src: &[u8],
    hist: &mut [u32; GREEN_SYMBOLS],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    for chunk in src.chunks(4096) {
        check_cancelled(cancel)?;
        for &lit in chunk {
            hist[usize::from(lit)] += 1;
        }
    }
    Ok(())
}

fn copy_token(len: usize, dist: usize) -> (usize, u32, u32) {
    let (sym, lbits, lval) = prefix(len as u32);
    let (_, dbits, dval) = prefix(PLANE_CODES[dist]);
    let dcode = (dist as u32).reverse_bits() >> 29;
    let extra = lval | (dcode | (dval << 3)) << lbits;
    (256 + sym as usize, extra, lbits + 3 + dbits)
}

pub(super) fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let (a_blk, _) = a.as_chunks::<16>();
    let (b_blk, _) = b.as_chunks::<16>();
    let shared = a_blk.iter().zip(b_blk).take_while(|(x, y)| x == y).count() * 16;
    shared
        + a[shared..]
            .iter()
            .zip(&b[shared..])
            .take_while(|(x, y)| x == y)
            .count()
}

pub(super) fn padded_len(bits: usize) -> usize {
    bits.div_ceil(8).next_multiple_of(2)
}

pub(super) fn symbol_bits<const N: usize>(codes: &[(u32, u32); N], hist: &[u32; N]) -> usize {
    codes
        .iter()
        .zip(hist)
        .map(|(&(_, len), &n)| len as usize * n as usize)
        .sum()
}

/// VP8L prefix-code `(symbol, extra_bits, extra_value)`.
pub(super) fn prefix(val: u32) -> (u32, u32, u32) {
    let v = val - 1;
    if v < 4 {
        return (v, 0, 0);
    }
    let extra = v.ilog2() - 1;
    (
        2 * (extra + 1) + ((v >> extra) & 1),
        extra,
        v & ((1 << extra) - 1),
    )
}

fn fixed_codes(src: &[u8]) -> [(u32, u32); GREEN_SYMBOLS] {
    let mut hist = [0u32; 256];
    let stride = (src.len() / 8192).max(1);
    for &s in src.iter().step_by(stride) {
        hist[usize::from(s)] += 1;
    }
    let mut syms = std::array::from_fn::<_, 256, _>(|i| i);
    syms.sort_unstable_by_key(|&s| (Reverse(hist[s]), s));
    let mut lens = [6; GREEN_SYMBOLS];
    lens[..256].fill(9);
    for &s in &syms[..8] {
        lens[s] = 6;
    }
    for &s in &syms[8..16] {
        lens[s] = 8;
    }
    codes(&lens)
}

pub(super) fn code_lengths<const N: usize>(hist: &[u32; N], limit: u32) -> Option<[u32; N]> {
    let syms = hist.iter().filter(|&&n| n != 0).count();
    if syms < 2 || syms > 1 << limit {
        return None;
    }
    // Raise frequency floor only if the unconstrained tree exceeds `limit`.
    let mut floor = 1;
    loop {
        if let Some(lens) = huffman_lengths(hist, limit, floor) {
            return Some(lens);
        }
        floor *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::{code_lengths, huffman_lengths};

    #[test]
    fn huffman_depth_cap() {
        let hist = std::array::from_fn::<_, 20, _>(|i| 1 << i);
        assert!(huffman_lengths(&hist, 15, 1).is_none());
        let lens = code_lengths(&hist, 15).unwrap();
        assert!(lens.iter().all(|len| (1..=15).contains(len)));
        assert_eq!(lens.iter().map(|len| 1 << (15 - len)).sum::<u32>(), 1 << 15);
    }
}

fn huffman_lengths<const N: usize>(hist: &[u32; N], limit: u32, floor: u64) -> Option<[u32; N]> {
    let mut nodes: BinaryHeap<_> = hist
        .iter()
        .enumerate()
        .filter(|&(_, &n)| n != 0)
        .map(|(i, &n)| Reverse((u64::from(n).max(floor), i)))
        .collect();
    let mut branches = Vec::with_capacity(nodes.len() - 1);
    while nodes.len() > 1 {
        let Reverse((n, left)) = nodes.pop().unwrap();
        let mut right = nodes.peek_mut().unwrap();
        let parent = N + branches.len();
        branches.push((left, right.0.1));
        *right = Reverse((n + right.0.0, parent));
    }

    let mut lens = [0; N];
    let mut stack = vec![(N + branches.len() - 1, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > limit {
            return None;
        }
        if node < N {
            lens[node] = depth;
        } else {
            let (left, right) = branches[node - N];
            stack.extend([(left, depth + 1), (right, depth + 1)]);
        }
    }
    Some(lens)
}

pub(super) fn codes<const N: usize>(lens: &[u32; N]) -> [(u32, u32); N] {
    let mut counts = [0; 16];
    for &len in lens {
        counts[len as usize] += u32::from(len != 0);
    }
    let mut next = [0u32; 16];
    for len in 1..next.len() {
        next[len] = (next[len - 1] + counts[len - 1]) << 1;
    }
    lens.map(|len| {
        if len == 0 {
            return (0, 0);
        }
        let code = &mut next[len as usize];
        let rev = code.reverse_bits() >> (32 - len);
        *code += 1;
        (rev, len)
    })
}

#[derive(Clone)]
pub(super) struct Bits {
    buf: Vec<u8>,
    pending: u64,
    count: u32,
}

impl Bits {
    pub(super) fn new(stream: Stream) -> Self {
        let mut bits = Self::bare(stream);
        bits.write(0, 2); // no color cache or meta Huffman groups
        bits
    }

    pub(super) fn header(method: u8) -> Self {
        Self {
            buf: vec![method],
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
            bits.write(3, 3); // 32x32 tiles
            bits.write(0, 1); // no color cache
            for val in [13, 0, 0, 0, 0] {
                bits.write_single_tree(val);
            }
        }
        bits.write(0, 1); // end of transforms
        bits
    }

    pub(super) fn palette(method: u8, palette: &[u8]) -> Self {
        let mut bits = Self {
            buf: vec![method],
            pending: 0,
            count: 0,
        };
        bits.write(7, 3);
        bits.write((palette.len() - 1) as u32, 8);
        bits.write(0, 1);
        let mut deltas = [0; 16];
        let mut hist = [0; GREEN_SYMBOLS];
        let mut prev = 0;
        for (&val, delta) in palette.iter().zip(&mut deltas) {
            *delta = val.wrapping_sub(prev);
            prev = val;
            hist[*delta as usize] += 1;
        }
        let tree = if let Some(lens) = code_lengths(&hist, 15) {
            bits.write_green_tree(&lens);
            Some(codes(&lens))
        } else {
            bits.write_single_tree(u32::from(deltas[0]));
            None
        };
        for val in [0, 0, 255, 0] {
            bits.write_single_tree(val);
        }
        if let Some(tree) = tree {
            for &sym in &deltas[..palette.len()] {
                let (code, len) = tree[sym as usize];
                bits.write(code, len);
            }
        }
        bits.write(0, 3);
        bits
    }

    pub(super) fn len(&self) -> usize {
        self.buf.len() * 8 + self.count as usize
    }

    pub(super) fn reserve_bytes(&mut self, total: usize) {
        self.buf.reserve_exact(total - self.buf.len());
    }

    pub(super) fn finish(mut self) -> Vec<u8> {
        let tail = self.pending.to_le_bytes();
        self.buf
            .extend_from_slice(&tail[..self.count.div_ceil(8) as usize]);
        self.buf
    }

    pub(super) fn write_token(&mut self, (code, len): (u32, u32), extra: u32, nbits: u32) {
        self.write(code, len);
        self.write(extra, nbits);
    }

    pub(super) fn write_literals(
        &mut self,
        src: &[u8],
        green: &[(u32, u32); GREEN_SYMBOLS],
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), Error> {
        for chunk in src.chunks(4096) {
            check_cancelled(cancel)?;
            for &s in chunk {
                let (code, nbits) = green[usize::from(s)];
                self.write(code, nbits);
            }
        }
        Ok(())
    }

    pub(super) fn write_green_tree(&mut self, lens: &[u32; GREEN_SYMBOLS]) {
        self.write_tree(lens);
    }

    pub(super) fn write_single_tree(&mut self, sym: u32) {
        self.write(1, 2);
        self.write(u32::from(sym > 1), 1);
        self.write(sym, if sym > 1 { 8 } else { 1 });
    }

    pub(super) fn write_tree(&mut self, lens: &[u32]) {
        let mut hist = [0; 19];
        for &len in lens {
            hist[len as usize] += 1;
        }
        let plain = LengthTree::new(&hist);
        let mut compact_hist = [0; 19];
        let mut extra = 0;
        visit_length_runs(lens, |sym, _, w| {
            compact_hist[sym] += 1;
            extra += w as usize;
        });
        if compact_hist != hist {
            let compact = LengthTree::new(&compact_hist);
            if compact.bits(&compact_hist) + extra < plain.bits(&hist) {
                compact.write(self);
                visit_length_runs(lens, |sym, val, w| {
                    self.write_token(compact.codes[sym], val, w);
                });
                return;
            }
        }
        plain.write(self);
        for &len in lens {
            let (code, w) = plain.codes[len as usize];
            self.write(code, w);
        }
    }

    pub(super) fn write_distance_tree(
        &mut self,
        hist: &[u32; DISTANCE_SYMBOLS],
    ) -> [(u32, u32); DISTANCE_SYMBOLS] {
        if let Some(lens) = code_lengths(hist, 15) {
            self.write_tree(&lens);
            codes(&lens)
        } else {
            let sym = hist.iter().position(|&n| n != 0).unwrap_or(0);
            self.write_single_tree(sym as u32);
            [(0, 0); DISTANCE_SYMBOLS]
        }
    }

    pub(super) fn write_unused_channels(&mut self) {
        for sym in [0, 0, 255] {
            self.write_single_tree(sym);
        }
    }

    fn write_fixed_distance_tree(&mut self) {
        self.write(0, 1);
        self.write(2, 4);
        for len in [0, 0, 1, 0, 0, 1] {
            self.write(len, 3);
        }
        self.write(0, 1);
        for sym in 0..DISTANCE_SYMBOLS as u32 {
            self.write(
                u32::from(PLANE_CODES.iter().any(|&code| prefix(code).0 == sym)),
                1,
            );
        }
    }

    pub(super) fn write(&mut self, val: u32, nbits: u32) {
        self.pending |= u64::from(val) << self.count;
        self.count += nbits;
        if self.count >= 32 {
            let word = self.pending as u32;
            self.buf.extend_from_slice(&word.to_le_bytes());
            self.pending >>= 32;
            self.count -= 32;
        }
    }
}

struct LengthTree {
    lens: [u32; 19],
    codes: [(u32, u32); 19],
    entries: usize,
}

impl LengthTree {
    fn new(hist: &[u32; 19]) -> Self {
        let lens = code_lengths(hist, 7).unwrap_or_else(|| hist.map(|n| u32::from(n != 0)));
        let entries = (1 + CODE_LENGTH_ORDER
            .iter()
            .rposition(|&i| lens[i] != 0)
            .unwrap())
        .max(4);
        let codes = if hist.iter().filter(|&&n| n != 0).count() == 1 {
            [(0, 0); 19]
        } else {
            codes(&lens)
        };
        Self {
            lens,
            codes,
            entries,
        }
    }

    fn bits(&self, hist: &[u32; 19]) -> usize {
        6 + self.entries * 3 + symbol_bits(&self.codes, hist)
    }

    fn write(&self, bits: &mut Bits) {
        bits.write(0, 1);
        bits.write((self.entries - 4) as u32, 4);
        for &i in &CODE_LENGTH_ORDER[..self.entries] {
            bits.write(self.lens[i], 3);
        }
        bits.write(0, 1);
    }
}

fn visit_length_runs(mut lens: &[u32], mut write: impl FnMut(usize, u32, u32)) {
    while let Some(&len) = lens.first() {
        if len == 0 {
            let zeros = lens.iter().take(138).take_while(|&&n| n == 0).count();
            if zeros >= 11 {
                write(18, zeros as u32 - 11, 7);
                lens = &lens[zeros..];
                continue;
            }
            if zeros >= 3 {
                write(17, zeros as u32 - 3, 3);
                lens = &lens[zeros..];
                continue;
            }
        }
        write(len as usize, 0, 0);
        lens = &lens[1..];
    }
}
