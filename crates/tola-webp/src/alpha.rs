//! Lossless alpha payloads for extended WebP images.

use std::io::{self, Write};

pub(crate) fn encode(rgba: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if is_opaque(rgba) {
        return None;
    }
    let pixels = rgba.as_chunks::<4>().0;
    let mut alpha: Vec<_> = std::iter::once(0)
        .chain(pixels.iter().map(|pixel| pixel[3]))
        .collect();
    filter_vertical(&mut alpha[1..], width);
    Some(match compress(&alpha[1..], width, height) {
        Some(compressed) => compressed,
        _ => {
            for (value, pixel) in alpha[1..].iter_mut().zip(pixels) {
                *value = pixel[3];
            }
            alpha
        }
    })
}

fn is_opaque(rgba: &[u8]) -> bool {
    // The alpha byte is the top byte of each native-endian pixel word.
    const ALPHA: u64 = u64::from_ne_bytes([0, 0, 0, 255, 0, 0, 0, 255]);
    let (words, remainder) = rgba.as_chunks::<8>();
    words
        .iter()
        .all(|word| u64::from_ne_bytes(*word) & ALPHA == ALPHA)
        && remainder
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
}

fn filter_vertical(alpha: &mut [u8], width: usize) {
    // Reverse traversal preserves the original top and left predictors.
    for position in (width..alpha.len()).rev() {
        alpha[position] = alpha[position].wrapping_sub(alpha[position - width]);
    }
    for position in (1..width).rev() {
        alpha[position] = alpha[position].wrapping_sub(alpha[position - 1]);
    }
}

/// Serialize `differences` as an ALPH method 1 payload in one pass.
///
/// The pinned `image-webp` encoder writes the RIFF header, its VP8L chunk header, and the
/// five-byte VP8L image header before the frame body; method 1 replaces those five bytes with
/// its method byte, so the body is copied straight into the payload instead of being moved
/// afterwards. Returns `None` unless the payload is strictly smaller than the raw fallback.
fn compress(differences: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let mut sink = AlphaSink::new(differences.len());
    let mut params = image_webp::EncoderParams::default();
    // ALPH supplies the vertical prediction without a VP8L predictor-transform header.
    params.use_predictor_transform = false;
    let mut encoder = image_webp::WebPEncoder::new(&mut sink);
    encoder.set_params(params);
    encoder
        .encode(
            differences,
            width as u32,
            height as u32,
            image_webp::ColorType::L8,
        )
        .ok()?;
    sink.finish()
}

/// Collects the ALPH payload from the pinned `image-webp` container layout while it is written.
struct AlphaSink {
    header: [u8; 20],
    written: usize,
    payload: Vec<u8>,
    raw_len: usize,
}

impl AlphaSink {
    /// `raw_len` is the raw fallback payload length, including its method byte.
    fn new(raw_len: usize) -> Self {
        Self {
            header: [0; 20],
            written: 0,
            payload: vec![9], // ALPH compression 1, vertical filter 2
            raw_len,
        }
    }

    /// The ALPH payload if the stream is one unmodified `image-webp` VP8L container.
    fn finish(mut self) -> Option<Vec<u8>> {
        if self.written < 25
            || &self.header[..4] != b"RIFF"
            || &self.header[8..12] != b"WEBP"
            || &self.header[12..16] != b"VP8L"
        {
            return None;
        }
        let length = u32::from_le_bytes(self.header[16..20].try_into().ok()?) as usize;
        if length <= 5 || self.written != 20 + length + (length & 1) {
            return None;
        }
        let body = length - 5;
        if self.payload.len() != 1 + body + (length & 1) || 1 + body > self.raw_len {
            return None;
        }
        self.payload.truncate(1 + body);
        Some(self.payload)
    }
}

impl Write for AlphaSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let start = self.written;
        self.written += buffer.len();
        if start < self.header.len() {
            let copied = (self.header.len() - start).min(buffer.len());
            self.header[start..start + copied].copy_from_slice(&buffer[..copied]);
        }
        if let Some(body) = buffer.get(25usize.saturating_sub(start)..) {
            // Chunk padding is included here, so this check is conservative; `finish`
            // makes the strict comparison against the raw payload.
            if self.payload.len() - 1 + body.len() > self.raw_len {
                return Err(io::Error::other(
                    "alpha payload cannot beat the raw fallback",
                ));
            }
            self.payload.reserve(body.len() + 1);
            self.payload.extend_from_slice(body);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-optimization extraction: write the full container, then scan it for VP8L.
    fn container_payload(differences: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
        let mut file = Vec::new();
        let mut params = image_webp::EncoderParams::default();
        params.use_predictor_transform = false;
        let mut encoder = image_webp::WebPEncoder::new(&mut file);
        encoder.set_params(params);
        encoder
            .encode(
                differences,
                width as u32,
                height as u32,
                image_webp::ColorType::L8,
            )
            .ok()?;
        let mut position = 12;
        while position + 8 <= file.len() {
            let fourcc = &file[position..position + 4];
            let length =
                u32::from_le_bytes(file[position + 4..position + 8].try_into().ok()?) as usize;
            let end = position + 8 + length;
            let payload = file.get(position + 8..end)?;
            if fourcc == b"VP8L" {
                if payload.len() <= 5 {
                    return None;
                }
                file.copy_within(position + 13..end, 1);
                file.truncate(length - 4);
                file[0] = 9;
                return Some(file);
            }
            position += 8 + length + (length & 1);
        }
        None
    }

    #[test]
    fn payload_matches_container_scan() {
        for (width, height) in [(1, 1), (1, 7), (8, 3), (17, 19), (32, 33)] {
            for pattern in 0..4u8 {
                let differences: Vec<u8> = (0..width * height)
                    .map(|index| match pattern {
                        0 => 0,
                        1 => 255,
                        2 => (index % 17) as u8,
                        _ => (index as u8).wrapping_mul(37),
                    })
                    .collect();
                let limit = differences.len() + 1;
                let actual =
                    compress(&differences, width, height).filter(|payload| payload.len() < limit);
                let expected = container_payload(&differences, width, height)
                    .filter(|payload| payload.len() < limit);
                assert_eq!(actual, expected, "{width}x{height} pattern {pattern}");
            }
        }
    }
}
