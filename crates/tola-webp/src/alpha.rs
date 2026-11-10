//! Lossless alpha payloads for extended WebP images.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{self, Write};

use crate::Error;
use crate::encoder::check_cancelled;

const RAW_ALPHA: u8 = 0;
const VP8L_ALPHA: u8 = 1;
const VERTICAL_FILTER: u8 = 2 << 2;
const PIXELS_PER_CHECK: usize = 16 * 1024;

pub(crate) fn encode(
    rgba: &[u8],
    width: usize,
    height: usize,
    has_transparency: Option<bool>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    check_cancelled(cancelled)?;
    if has_transparency == Some(false) {
        return Ok(None);
    }

    let raw_len = width * height + 1;
    let first = rgba[3];
    if rgba.last() == Some(&first)
        && (first != u8::MAX || has_transparency.is_none())
        && uniform_alpha(rgba, first, cancelled)?
    {
        if first == u8::MAX {
            return Ok(None);
        }
        // The 1×1 frame has one Huffman symbol per channel and no per-pixel bits.
        // ALPH supplies the actual dimensions and decodes that symbol for every pixel.
        let alpha = compress(&[first], 1, 1, VP8L_ALPHA, raw_len).unwrap_or_else(|| {
            let mut raw = vec![first; raw_len];
            raw[0] = RAW_ALPHA;
            raw
        });
        check_cancelled(cancelled)?;
        return Ok(Some(alpha));
    }

    let pixels = rgba.as_chunks::<4>().0;
    let mut rows = pixels.chunks_exact(width);
    let first_row = rows.next().expect("the dimensions are nonzero");
    let mut alpha = Vec::with_capacity(raw_len);
    alpha.extend([RAW_ALPHA, first]);
    // The first row predicts from the left; subsequent rows predict from above.
    alpha.extend(
        first_row
            .windows(2)
            .map(|pair| pair[1][3].wrapping_sub(pair[0][3])),
    );
    let mut above = first_row;
    for row in rows {
        check_cancelled(cancelled)?;
        alpha.extend(
            row.iter()
                .zip(above)
                .map(|(pixel, top)| pixel[3].wrapping_sub(top[3])),
        );
        above = row;
    }

    check_cancelled(cancelled)?;
    let filtered = &alpha[1..];
    if !cannot_beat_raw(filtered, cancelled)? {
        let compressed = compress(
            filtered,
            width,
            height,
            VP8L_ALPHA | VERTICAL_FILTER,
            raw_len,
        );
        check_cancelled(cancelled)?;
        if let Some(compressed) = compressed {
            return Ok(Some(compressed));
        }
    }

    for (raw, pixels) in alpha[1..]
        .chunks_mut(PIXELS_PER_CHECK)
        .zip(pixels.chunks(PIXELS_PER_CHECK))
    {
        check_cancelled(cancelled)?;
        for (value, pixel) in raw.iter_mut().zip(pixels) {
            *value = pixel[3];
        }
    }
    Ok(Some(alpha))
}

fn cannot_beat_raw(samples: &[u8], cancelled: &(dyn Fn() -> bool + Sync)) -> Result<bool, Error> {
    const BYTE_VALUES: usize = u8::MAX as usize + 1;

    // A varied window makes the full count worthwhile. It never decides the encoding.
    let start = samples.len().saturating_sub(BYTE_VALUES) / 2;
    let Some(window) = samples.get(start..start + BYTE_VALUES) else {
        return Ok(false);
    };
    let mut seen = [false; BYTE_VALUES];
    for &value in window {
        seen[usize::from(value)] = true;
    }
    if seen.into_iter().filter(|&seen| seen).count() < BYTE_VALUES / 2 {
        return Ok(false);
    }

    // The pinned VP8L encoder emits a literal and, for a repeated run, at least one copy.
    // Keep only those first symbols and merge all copies into one symbol. The actual tree's
    // literal codes plus its shortest copy code would encode this histogram at no greater cost.
    let mut counts = [0u32; BYTE_VALUES + 1];
    let mut previous = samples[0];
    let mut copied = false;
    counts[usize::from(previous)] = 1;
    for chunk in samples[1..].chunks(PIXELS_PER_CHECK) {
        check_cancelled(cancelled)?;
        for &value in chunk {
            if value != previous {
                counts[usize::from(value)] += 1;
                previous = value;
                copied = false;
            } else if !copied {
                counts[BYTE_VALUES] += 1;
                copied = true;
            }
        }
    }

    // Unrestricted Huffman merges give the minimum symbol cost, ignoring headers and extra bits.
    let mut weights: BinaryHeap<_> = counts
        .into_iter()
        .filter(|&count| count != 0)
        .map(Reverse)
        .collect();
    let mut bits = 0u64;
    while let Some(Reverse(left)) = weights.pop() {
        let Some(mut right) = weights.peek_mut() else {
            break;
        };
        right.0 += left;
        bits += u64::from(right.0);
    }
    let minimum_len = 1 + bits.div_ceil(8);
    Ok(minimum_len.next_multiple_of(2) >= (samples.len() as u64 + 1).next_multiple_of(2))
}

fn uniform_alpha(
    rgba: &[u8],
    value: u8,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<bool, Error> {
    const MASK: u64 = u64::from_ne_bytes([0, 0, 0, 255, 0, 0, 0, 255]);
    let alpha = u64::from_ne_bytes([0, 0, 0, value, 0, 0, 0, value]);
    for pixels in rgba.chunks(4 * PIXELS_PER_CHECK) {
        check_cancelled(cancelled)?;
        let (pairs, tail) = pixels.as_chunks::<8>();
        if pairs
            .iter()
            .any(|pair| u64::from_ne_bytes(*pair) & MASK != alpha)
            || tail.get(3).is_some_and(|&last| last != value)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn compress(
    samples: &[u8],
    width: usize,
    height: usize,
    method: u8,
    raw_len: usize,
) -> Option<Vec<u8>> {
    let mut sink = AlphaSink::new(method, raw_len);
    let mut params = image_webp::EncoderParams::default();
    params.use_predictor_transform = false;
    let mut encoder = image_webp::WebPEncoder::new(&mut sink);
    encoder.set_params(params);
    encoder
        .encode(
            samples,
            width as u32,
            height as u32,
            image_webp::ColorType::L8,
        )
        .ok()?;
    sink.finish()
}

const RIFF_HEADER_LEN: usize = 20;
const VP8L_HEADER_LEN: usize = 5;
const IMAGE_STREAM_START: usize = RIFF_HEADER_LEN + VP8L_HEADER_LEN;

/// Strip the pinned encoder's RIFF and VP8L headers. ALPH replaces them with its method byte.
struct AlphaSink {
    header: [u8; RIFF_HEADER_LEN],
    written: usize,
    frame_len: Option<usize>,
    payload: Vec<u8>,
    method: u8,
    raw_len: usize,
}

impl AlphaSink {
    fn new(method: u8, raw_len: usize) -> Self {
        Self {
            header: [0; RIFF_HEADER_LEN],
            written: 0,
            frame_len: None,
            payload: Vec::new(),
            method,
            raw_len,
        }
    }

    fn start_payload(&mut self) -> io::Result<()> {
        if &self.header[..4] != b"RIFF" || &self.header[8..16] != b"WEBPVP8L" {
            return Err(io::Error::other("unexpected VP8L container"));
        }
        let frame_len = u32::from_le_bytes(self.header[16..20].try_into().unwrap()) as usize;
        if frame_len <= VP8L_HEADER_LEN {
            return Err(io::Error::other("missing VP8L image stream"));
        }
        let payload_len = frame_len - VP8L_HEADER_LEN + 1;
        if payload_len.next_multiple_of(2) >= self.raw_len.next_multiple_of(2) {
            return Err(io::Error::other(
                "compressed alpha is no smaller than raw alpha",
            ));
        }
        let riff_len = u32::from_le_bytes(self.header[4..8].try_into().unwrap()) as usize;
        if riff_len != RIFF_HEADER_LEN - 8 + frame_len.next_multiple_of(2) {
            return Err(io::Error::other("unexpected VP8L container length"));
        }
        self.frame_len = Some(frame_len);
        self.payload.reserve_exact(payload_len);
        self.payload.push(self.method);
        Ok(())
    }

    fn finish(self) -> Option<Vec<u8>> {
        let frame_len = self.frame_len?;
        (self.written == RIFF_HEADER_LEN + frame_len.next_multiple_of(2)
            && self.payload.len() == frame_len - VP8L_HEADER_LEN + 1)
            .then_some(self.payload)
    }
}

impl Write for AlphaSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let start = self.written;
        let end = start + buffer.len();
        if start < RIFF_HEADER_LEN {
            let header_end = end.min(RIFF_HEADER_LEN);
            self.header[start..header_end].copy_from_slice(&buffer[..header_end - start]);
            if header_end == RIFF_HEADER_LEN {
                self.start_payload()?;
            }
        }
        if let Some(frame_len) = self.frame_len
            && let Some(body) = buffer.get(IMAGE_STREAM_START.saturating_sub(start)..)
        {
            let remaining = frame_len - VP8L_HEADER_LEN + 1 - self.payload.len();
            self.payload
                .extend_from_slice(&body[..remaining.min(body.len())]);
        }
        self.written = end;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{VP8L_ALPHA, cannot_beat_raw, compress};

    #[test]
    fn raw_bound_accounts_for_samples_outside_the_probe() {
        let mut samples: Vec<_> = (0..=u8::MAX).cycle().take(256 * 16).collect();
        assert!(cannot_beat_raw(&samples, &|| false).unwrap());
        assert!(compress(&samples, 256, 16, VP8L_ALPHA, samples.len() + 1).is_none());

        let middle = (samples.len() - 256) / 2;
        samples[..middle].fill(0);
        samples[middle + 256..].fill(0);
        assert!(!cannot_beat_raw(&samples, &|| false).unwrap());
        assert!(compress(&samples, 256, 16, VP8L_ALPHA, samples.len() + 1).is_some());
    }

    #[test]
    fn alpha_round_trips() {
        for (width, height) in [(1, 1), (1, 7), (8, 3), (17, 19), (32, 33)] {
            for pattern in 0..4 {
                let rgba: Vec<u8> = (0..width * height)
                    .flat_map(|index| {
                        let alpha = match pattern {
                            0 => 0,
                            1 => 170,
                            2 => (index % 17) as u8,
                            _ => (index as u8).wrapping_mul(37),
                        };
                        [37, 123, 211, alpha]
                    })
                    .collect();
                let webp = crate::encode_rgba(&rgba, width, height, 75).unwrap();
                let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
                assert_eq!(decoded.dimensions(), (width, height));
                for (actual, expected) in decoded.pixels().zip(rgba.as_chunks::<4>().0) {
                    assert_eq!(
                        actual[3], expected[3],
                        "{width}x{height}, pattern {pattern}"
                    );
                }
            }
        }
    }
}
