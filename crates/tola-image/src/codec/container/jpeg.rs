use anyhow::{Context, Result, ensure};

use crate::cancellation::Cancellation;

pub(in crate::codec) fn jpeg_has_icc(
    bytes: &[u8],
    cancellation: &dyn Cancellation,
) -> Result<bool> {
    let mut offset = 2usize;
    let mut found = false;
    while offset < bytes.len() {
        cancellation.ensure_active()?;
        ensure!(
            bytes[offset] == 0xff,
            "the JPEG image is damaged or incomplete; re-export it"
        );
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes
            .get(offset)
            .context("the JPEG image is damaged or incomplete; re-export it")?;
        offset += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let length_bytes = bytes
            .get(offset..offset + 2)
            .context("the JPEG image is damaged or incomplete; re-export it")?;
        let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
        ensure!(
            length >= 2,
            "the JPEG image is damaged or incomplete; re-export it"
        );
        let segment = bytes
            .get(offset + 2..offset + length)
            .context("the JPEG image is damaged or incomplete; re-export it")?;
        if marker == 0xe2 && segment.starts_with(b"ICC_PROFILE\0") {
            found = true;
        }
        offset += length;
    }
    Ok(found)
}
