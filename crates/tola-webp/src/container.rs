//! The RIFF container around one VP8 frame.

use std::io::{self, IoSlice, Write};

use crate::vp8::Frame;

pub(crate) struct WebP {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) frame: Frame,
    pub(crate) alpha: Option<Vec<u8>>,
}

impl WebP {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + self.riff_size());
        self.write_to(&mut bytes)
            .expect("writing to a Vec cannot fail");
        bytes
    }

    fn riff_size(&self) -> usize {
        let alpha = self
            .alpha
            .as_ref()
            .map_or(0, |alpha| chunk_size(10) + chunk_size(alpha.len()));
        4 + alpha + chunk_size(self.frame.len())
    }

    pub(crate) fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        let mut header = *b"RIFF\0\0\0\0WEBP";
        header[4..8].copy_from_slice(&(self.riff_size() as u32).to_le_bytes());
        let alpha_chunks = self.alpha.as_ref().map(|alpha| {
            let vp8x = vp8x_payload(self.width, self.height);
            (
                chunk_header(b"VP8X", vp8x.len()),
                vp8x,
                chunk_header(b"ALPH", alpha.len()),
                alpha,
            )
        });
        let [vp8x_header, vp8x, alpha_header, alpha] = alpha_chunks.as_ref().map_or(
            [&[][..]; 4],
            |(vp8x_header, vp8x, alpha_header, alpha)| {
                [vp8x_header, vp8x, alpha_header, alpha.as_slice()]
            },
        );
        let [frame_header, modes, residual] = self.frame.chunks();
        let frame_len = self.frame.len();
        let frame_chunk = chunk_header(b"VP8 ", frame_len);
        let padding = [0];
        write_all_slices(
            out,
            &mut [
                IoSlice::new(&header),
                IoSlice::new(vp8x_header),
                IoSlice::new(vp8x),
                IoSlice::new(alpha_header),
                IoSlice::new(alpha),
                IoSlice::new(&padding[..alpha.len() & 1]),
                IoSlice::new(&frame_chunk),
                IoSlice::new(frame_header),
                IoSlice::new(modes),
                IoSlice::new(residual),
                IoSlice::new(&padding[..frame_len & 1]),
            ],
        )
    }
}

fn vp8x_payload(width: u32, height: u32) -> [u8; 10] {
    let mut payload = [0u8; 10];
    payload[0] = 0x10; // alpha present
    payload[4..7].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload[7..10].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
    payload
}

pub(crate) const fn chunk_size(payload: usize) -> usize {
    8 + payload + (payload & 1)
}

fn chunk_header(fourcc: &[u8; 4], len: usize) -> [u8; 8] {
    let mut header = [0; 8];
    header[..4].copy_from_slice(fourcc);
    header[4..].copy_from_slice(&(len as u32).to_le_bytes());
    header
}

fn write_all_slices(out: &mut impl Write, mut parts: &mut [IoSlice<'_>]) -> io::Result<()> {
    while !parts.is_empty() {
        match out.write_vectored(parts) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => IoSlice::advance_slices(&mut parts, written),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn riff_chunks_preserve_alignment() {
        for residual in [vec![1, 2, 3], vec![1, 2, 3, 4]] {
            let frame = Frame::new(1, 1, Vec::new(), residual.clone()).unwrap();
            let file = WebP {
                width: 1,
                height: 1,
                frame,
                alpha: None,
            }
            .into_bytes();
            let payload = 10 + residual.len();
            assert_eq!(&file[..4], b"RIFF");
            assert_eq!(
                u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize,
                file.len() - 8
            );
            assert_eq!(&file[8..16], b"WEBPVP8 ");
            assert_eq!(
                u32::from_le_bytes(file[16..20].try_into().unwrap()) as usize,
                payload
            );
            assert_eq!(&file[30..20 + payload], &residual);
            assert_eq!(&file[20 + payload..], &vec![0; payload & 1]);
        }
    }
}
