use std::borrow::Cow;

use anyhow::{Context, Result, ensure};

use super::super::{ContainerMetadata, SampleColor};
use super::le32;

pub(in crate::codec) fn scan_bmp(bytes: &[u8]) -> Result<ContainerMetadata<'_>> {
    ensure!(
        bytes.len() >= 18,
        "the BMP image is damaged or incomplete; re-export it"
    );
    let header_length = le32(&bytes[14..18]) as usize;
    ensure!(
        14usize
            .checked_add(header_length)
            .is_some_and(|end| end <= bytes.len()),
        "the BMP image is damaged or incomplete; re-export it"
    );
    let mut metadata = ContainerMetadata::default();
    if header_length >= 108 {
        // LCS_sRGB and LCS_WINDOWS_COLOR_SPACE both describe sRGB samples; LCS_PROFILE_EMBEDDED
        // points at an ICC profile the file itself carries.
        let color_space = le32(&bytes[70..74]);
        match color_space {
            0x7352_4742 | 0x5769_6e20 => {}
            0x4d42_4544 if header_length >= 124 => {
                let offset = le32(&bytes[126..130]) as usize;
                let length = le32(&bytes[130..134]) as usize;
                ensure!(
                    length > 0,
                    "the BMP image has an empty color profile; re-export it with sRGB colors"
                );
                let start = 14usize
                    .checked_add(offset)
                    .context("the BMP image is damaged or incomplete; re-export it")?;
                let end = start
                    .checked_add(length)
                    .context("the BMP image is damaged or incomplete; re-export it")?;
                metadata.color = SampleColor::StoredIcc(Cow::Borrowed(
                    bytes
                        .get(start..end)
                        .context("the BMP image is damaged or incomplete; re-export it")?,
                ));
            }
            _ => {
                metadata.color = SampleColor::Unsupported(
                    "the BMP image uses a color profile Tola cannot read; convert the image to sRGB",
                )
            }
        }
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageFormat;
    use crate::OutputFormat;
    use crate::codec::test_support::*;
    use crate::codec::{inspect, render};
    use image::ExtendedColorType;

    #[test]
    fn bmp_pixels_remain_rgb_when_re_encoded() {
        let mut source = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut source)
            .encode(&[255, 0, 0, 0, 0, 255], 2, 1, ExtendedColorType::Rgb8)
            .unwrap();
        let metadata = inspect(&source).unwrap();
        assert_eq!(
            (metadata.width, metadata.height, metadata.format),
            (2, 1, ImageFormat::Bmp)
        );
        let output = render(
            &source,
            &recipe(&source, 2, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        assert_eq!(rgba(&output), [255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(!inspect(&output).unwrap().has_alpha);
    }
    /// One 24-bit BMP with a V5 header, laid out the way Windows writes one: the file header, the
    /// 124-byte header, then any embedded profile, then bottom-up rows padded to four bytes.
    fn bmp_v5(pixels: &[u8], width: u32, height: u32, color_space: u32, profile: &[u8]) -> Vec<u8> {
        let header = 14 + 124;
        let stride = (width as usize * 3).div_ceil(4) * 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(
            &((header + profile.len() + stride * height as usize) as u32).to_le_bytes(),
        );
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&((header + profile.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&124u32.to_le_bytes()); // bV5Size
        bytes.extend_from_slice(&width.to_le_bytes()); // bV5Width
        bytes.extend_from_slice(&height.to_le_bytes()); // bV5Height
        bytes.extend_from_slice(&1u16.to_le_bytes()); // bV5Planes
        bytes.extend_from_slice(&24u16.to_le_bytes()); // bV5BitCount
        bytes.extend_from_slice(&[0u8; 16]); // compression, image size, resolutions
        bytes.extend_from_slice(&[0u8; 8]); // colors used and important
        bytes.extend_from_slice(&[0u8; 16]); // channel masks
        bytes.extend_from_slice(&color_space.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 36]); // endpoints
        bytes.extend_from_slice(&[0u8; 12]); // gamma
        bytes.extend_from_slice(&0u32.to_le_bytes()); // intent
        let data_offset = if profile.is_empty() { 0u32 } else { 124 };
        bytes.extend_from_slice(&data_offset.to_le_bytes()); // bV5ProfileData
        bytes.extend_from_slice(&(profile.len() as u32).to_le_bytes()); // bV5ProfileSize
        bytes.extend_from_slice(&0u32.to_le_bytes()); // bV5Reserved
        bytes.extend_from_slice(profile);
        for row in (0..height as usize).rev() {
            bytes.extend_from_slice(&pixels[row * width as usize * 3..][..width as usize * 3]);
            bytes.extend_from_slice(&vec![0u8; stride - width as usize * 3]);
        }
        bytes
    }

    /// A V5 header names its color space the way Windows writes it, and an embedded profile is
    /// read from the offsets that header defines. A space Tola cannot read is reported here rather
    /// than resized as if it were sRGB, and a profile that is present is handed on whole.
    #[test]
    fn v5_colour_space_and_profile_are_read() {
        // Red, green, blue, and mid grey: the linear profile below moves the mid grey only.
        let pixels: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];

        let srgb_bytes = bmp_v5(&pixels, 2, 2, 0x7352_4742, &[]);
        let srgb = scan_bmp(&srgb_bytes).unwrap();
        assert!(
            matches!(srgb.color, SampleColor::Srgb),
            "sRGB needs no conversion"
        );

        let profile = linear_profile();
        let embedded_bytes = bmp_v5(&pixels, 2, 2, 0x4d42_4544, &profile);
        let embedded = scan_bmp(&embedded_bytes).unwrap();
        assert!(
            matches!(&embedded.color, SampleColor::StoredIcc(bytes) if bytes.as_ref() == profile.as_slice()),
            "the profile the header points at is handed on whole"
        );

        let unreadable_bytes = bmp_v5(&pixels, 2, 2, 0, &[]);
        let unreadable = scan_bmp(&unreadable_bytes).unwrap();
        assert!(
            matches!(unreadable.color, SampleColor::Unsupported(_)),
            "a color space Tola cannot read is reported, not guessed"
        );

        // The file itself decodes: a Windows-saved V5 header reaches the resampler rather than
        // being refused, and the space it names as unreadable is refused before any decoding.
        let resized = |source: &[u8]| {
            render(
                source,
                &recipe(source, 2, 2, OutputFormat::Png, None),
                &active,
            )
        };
        assert_eq!(rgba(&resized(&srgb_bytes).unwrap()), rgba(&srgb_bytes));
        assert!((185..=190).contains(&rgba(&resized(&embedded_bytes).unwrap())[12]));
        assert!(resized(&unreadable_bytes).is_err());
    }
}
