use super::Stream;
use super::repeated::{Bits, GREEN_SYMBOLS, code_lengths, codes, prefix, symbol_bits};
use crate::Error;
use crate::encoder::check_cancelled;

pub(super) fn encode(
    samples: &[u8],
    stream: Stream,
    limit: usize,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let mut counts = [0u32; GREEN_SYMBOLS];
    let mut extras = 0;
    tokens(samples, cancelled, |symbol, _, count| {
        counts[symbol] += 1;
        extras += count as usize;
    })?;
    check_cancelled(cancelled)?;
    let mut bits = Bits::new(stream);
    let tree = if let Some(lengths) = code_lengths(&counts, 15) {
        bits.write_green_tree(&lengths);
        codes(&lengths)
    } else {
        let only = counts.iter().position(|&n| n != 0).unwrap();
        if only >= 256 || counts.iter().filter(|&&n| n != 0).count() != 1 {
            return Ok(None);
        }
        bits.write_single_tree(only as u32);
        [(0, 0); GREEN_SYMBOLS]
    };
    for value in [0, 0, 255, 1] {
        bits.write_single_tree(value);
    }
    let size = bits.len() + extras + symbol_bits(&tree, &counts);
    let size = size.div_ceil(8);
    if size.next_multiple_of(2) >= limit.next_multiple_of(2) {
        return Ok(None);
    }
    bits.reserve_bytes(size);
    tokens(samples, cancelled, |sym, extra, count| {
        let (code, nb) = tree[sym];
        bits.write(code, nb);
        bits.write(extra, count);
    })?;
    Ok(Some(bits.finish()))
}

fn tokens(
    samples: &[u8],
    cancelled: &(dyn Fn() -> bool + Sync),
    mut write: impl FnMut(usize, u32, u32),
) -> Result<(), Error> {
    let mut at = 0;
    let mut check_at = 0;
    while let Some(&value) = samples.get(at) {
        if at >= check_at {
            check_cancelled(cancelled)?;
            check_at = at + 4096;
        }
        write(usize::from(value), 0, 0);
        let remaining = &samples[at + 1..];
        let rest = &remaining[..remaining.len().min(4096)];
        let mut run = 0;
        if rest.first() == Some(&value) {
            let (blocks, _) = rest.as_chunks::<16>();
            let block = [value; 16];
            run = blocks.iter().take_while(|&&sample| sample == block).count() * 16;
            run += rest[run..].iter().take_while(|&&p| p == value).count();
        }
        if run != 0 {
            let (code, bits, extra) = prefix(run as u32);
            write(256 + code as usize, extra, bits);
        }
        at += run + 1;
    }
    Ok(())
}
