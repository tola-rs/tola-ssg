use std::borrow::Cow;

use anyhow::{Context, Result, ensure};

use super::super::{ContainerMetadata, SampleColor};
use super::le32;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;

pub(in crate::codec) fn scan_webp<'a>(
    bytes: &'a [u8],
    cancellation: Option<&dyn Cancellation>,
) -> Result<ContainerMetadata<'a>> {
    ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "the WebP image is damaged or incomplete; re-export it"
    );
    ensure!(
        u64::from(le32(&bytes[4..8])) + 8 == bytes.len() as u64,
        "the WebP image is damaged or incomplete; re-export it"
    );
    let mut metadata = ContainerMetadata::default();
    let mut profile = None;
    let mut profile_declared = false;
    let mut offset = 12usize;
    while offset < bytes.len() {
        ensure_active_if_present(cancellation)?;
        let header = bytes
            .get(offset..offset + 8)
            .context("the WebP image is damaged or incomplete; re-export it")?;
        let length = le32(&header[4..8]) as usize;
        let end = offset
            .checked_add(8)
            .and_then(|start| start.checked_add(length))
            .context("the WebP image is damaged or incomplete; re-export it")?;
        let padded_end = end
            .checked_add(length & 1)
            .context("the WebP image is damaged or incomplete; re-export it")?;
        ensure!(
            padded_end <= bytes.len(),
            "the WebP image is damaged or incomplete; re-export it"
        );
        let data = &bytes[offset + 8..end];
        match &header[..4] {
            b"VP8 " => metadata.is_lossy = true,
            b"VP8X" => {
                ensure!(
                    data.len() == 10,
                    "the WebP image is damaged or incomplete; re-export it"
                );
                let width = 1 + u32::from_le_bytes([data[4], data[5], data[6], 0]);
                let height = 1 + u32::from_le_bytes([data[7], data[8], data[9], 0]);
                check_dimensions(width, height)?;
                metadata.is_animated |= data[0] & 2 != 0;
                profile_declared |= data[0] & 32 != 0;
            }
            b"ANIM" | b"ANMF" => metadata.is_animated = true,
            b"ICCP" => {
                ensure!(
                    profile.is_none(),
                    "the WebP image has more than one color profile; re-export it with sRGB colors"
                );
                profile_declared = true;
                profile = Some(data);
            }
            b"EXIF" => {
                ensure!(
                    metadata.exif.is_none(),
                    "the WebP image has more than one EXIF block; re-export it"
                );
                metadata.exif = Some(data);
            }
            _ => {}
        }
        offset = padded_end;
    }
    // A VP8X flag may name a profile the file does not carry; the pixel decoder reports that
    // absence when a decode reads it.
    metadata.color = match profile {
        Some(profile) => SampleColor::StoredIcc(Cow::Borrowed(profile)),
        None if profile_declared => SampleColor::DecoderIcc,
        None => SampleColor::Srgb,
    };
    Ok(metadata)
}
