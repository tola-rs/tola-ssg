//! The RIFF container around one VP8 frame.

/// Wrap one VP8 frame and optional alpha in an extended WebP container.
pub(crate) fn riff_webp(width: u32, height: u32, payload: &[u8], alpha: Option<&[u8]>) -> Vec<u8> {
    let alpha_size = alpha.map_or(0, |alpha| 18 + 8 + alpha.len() + (alpha.len() & 1));
    let size = 4 + alpha_size + 8 + payload.len() + (payload.len() & 1);
    let mut out = Vec::with_capacity(8 + size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    if let Some(alpha) = alpha {
        push_chunk(&mut out, b"VP8X", &vp8x_payload(width, height));
        push_chunk(&mut out, b"ALPH", alpha);
    }
    push_chunk(&mut out, b"VP8 ", payload);
    out
}

fn vp8x_payload(width: u32, height: u32) -> [u8; 10] {
    let mut payload = [0u8; 10];
    payload[0] = 0x10; // alpha present
    payload[4..7].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload[7..10].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
    payload
}

fn push_chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        out.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::riff_webp;

    #[test]
    fn riff_chunks_preserve_alignment() {
        for payload in [&[1, 2, 3][..], &[1, 2, 3, 4][..]] {
            let file = riff_webp(1, 1, payload, None);
            assert_eq!(&file[..4], b"RIFF");
            assert_eq!(
                u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize,
                file.len() - 8
            );
            assert_eq!(&file[8..16], b"WEBPVP8 ");
            assert_eq!(
                u32::from_le_bytes(file[16..20].try_into().unwrap()) as usize,
                payload.len()
            );
            assert_eq!(&file[20..20 + payload.len()], payload);
            assert_eq!(&file[20 + payload.len()..], &vec![0; payload.len() & 1]);
        }
    }
}
