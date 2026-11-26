use std::borrow::Cow;

use crate::{ImageError, ImageResult};

use super::super::{ContainerMetadata, SampleColor};
use super::le32;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;

/// How a damaged WebP reads to a site author.
const DAMAGED: &str = "the WebP image is damaged or incomplete; re-export it";

pub(in crate::codec) fn scan_webp<'a>(
    bytes: &'a [u8],
    cancel: Option<&dyn Cancellation>,
) -> ImageResult<ContainerMetadata<'a>> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(ImageError::unreadable(DAMAGED));
    }
    if u64::from(le32(&bytes[4..8])) + 8 != bytes.len() as u64 {
        return Err(ImageError::unreadable(DAMAGED));
    }
    let mut meta = ContainerMetadata::default();
    let mut icc = None;
    let mut icc_declared = false;
    let mut off = 12usize;
    while off < bytes.len() {
        ensure_active_if_present(cancel)?;
        let hdr = bytes
            .get(off..off + 8)
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        let len = le32(&hdr[4..8]) as usize;
        let end = off
            .checked_add(8)
            .and_then(|start| start.checked_add(len))
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        let padded_end = end
            .checked_add(len & 1)
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        if padded_end > bytes.len() {
            return Err(ImageError::unreadable(DAMAGED));
        }
        let data = &bytes[off + 8..end];
        match &hdr[..4] {
            b"VP8 " => meta.is_lossy = true,
            b"VP8X" => {
                if data.len() != 10 {
                    return Err(ImageError::unreadable(DAMAGED));
                }
                let w = 1 + u32::from_le_bytes([data[4], data[5], data[6], 0]);
                let h = 1 + u32::from_le_bytes([data[7], data[8], data[9], 0]);
                check_dimensions(w, h)?;
                meta.is_animated |= data[0] & 2 != 0;
                icc_declared |= data[0] & 32 != 0;
            }
            b"ANIM" | b"ANMF" => meta.is_animated = true,
            b"ICCP" => {
                if icc.is_some() {
                    return Err(ImageError::unreadable(
                        "the WebP image has more than one color profile; re-export it with sRGB colors",
                    ));
                }
                icc_declared = true;
                icc = Some(data);
            }
            b"EXIF" => {
                if meta.exif.is_some() {
                    return Err(ImageError::unreadable(
                        "the WebP image has more than one EXIF block; re-export it",
                    ));
                }
                meta.exif = Some(data);
            }
            _ => {}
        }
        off = padded_end;
    }
    // A VP8X flag may name a profile the file does not carry; the decoder reports that absence.
    meta.color = match icc {
        Some(icc) => SampleColor::StoredIcc(Cow::Borrowed(icc)),
        None if icc_declared => SampleColor::DecoderIcc,
        None => SampleColor::Srgb,
    };
    Ok(meta)
}
