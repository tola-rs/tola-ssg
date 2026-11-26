use std::borrow::Cow;

use crate::{ImageError, ImageResult};

use super::super::{ContainerMetadata, SampleColor};
use super::le32;

/// How a damaged BMP reads to a site author.
const DAMAGED: &str = "the BMP image is damaged or incomplete; re-export it";

pub(in crate::codec) fn scan_bmp(bytes: &[u8]) -> ImageResult<ContainerMetadata<'_>> {
    if bytes.len() < 18 {
        return Err(ImageError::unreadable(DAMAGED));
    }
    let hdr_len = le32(&bytes[14..18]) as usize;
    if 14usize
        .checked_add(hdr_len)
        .is_none_or(|end| end > bytes.len())
    {
        return Err(ImageError::unreadable(DAMAGED));
    }
    let mut meta = ContainerMetadata::default();
    if hdr_len >= 108 {
        // LCS_sRGB and LCS_WINDOWS_COLOR_SPACE both describe sRGB; LCS_PROFILE_EMBEDDED points at an ICC profile.
        let cs = le32(&bytes[70..74]);
        match cs {
            0x7352_4742 | 0x5769_6e20 => {}
            0x4d42_4544 if hdr_len >= 124 => {
                let off = le32(&bytes[126..130]) as usize;
                let len = le32(&bytes[130..134]) as usize;
                if len == 0 {
                    return Err(ImageError::unreadable(
                        "the BMP image has an empty color profile; re-export it with sRGB colors",
                    ));
                }
                let start = 14usize
                    .checked_add(off)
                    .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                let end = start
                    .checked_add(len)
                    .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                meta.color = SampleColor::StoredIcc(Cow::Borrowed(
                    bytes
                        .get(start..end)
                        .ok_or_else(|| ImageError::unreadable(DAMAGED))?,
                ));
            }
            _ => {
                meta.color = SampleColor::Unsupported(
                    "the BMP image uses a color profile Tola cannot read; convert the image to sRGB",
                )
            }
        }
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageFormat;
    use crate::NO_CANCELLATION;
    use crate::OutputFormat;
    use crate::codec::tests::test_support::*;
    use crate::codec::{inspect, render};
    use image::ExtendedColorType;

    #[test]
    fn bmp_rgb_roundtrip() {
        let mut src = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut src)
            .encode(&[255, 0, 0, 0, 0, 255], 2, 1, ExtendedColorType::Rgb8)
            .unwrap();
        let meta = inspect(&src).unwrap();
        assert_eq!(
            (meta.width, meta.height, meta.format),
            (2, 1, ImageFormat::Bmp)
        );
        let out = render_png(&src, 2, 1);
        assert_eq!(rgba(&out), [255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(!inspect(&out).unwrap().has_alpha);
    }

    fn bmp_v5(px: &[u8], w: u32, h: u32, cs: u32, icc: &[u8]) -> Vec<u8> {
        let hdr = 14 + 124;
        let stride = (w as usize * 3).div_ceil(4) * 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&((hdr + icc.len() + stride * h as usize) as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&((hdr + icc.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&124u32.to_le_bytes()); // bV5Size
        bytes.extend_from_slice(&w.to_le_bytes()); // bV5Width
        bytes.extend_from_slice(&h.to_le_bytes()); // bV5Height
        bytes.extend_from_slice(&1u16.to_le_bytes()); // bV5Planes
        bytes.extend_from_slice(&24u16.to_le_bytes()); // bV5BitCount
        bytes.extend_from_slice(&[0u8; 16]); // compression, image size, resolutions
        bytes.extend_from_slice(&[0u8; 8]); // colors used and important
        bytes.extend_from_slice(&[0u8; 16]); // channel masks
        bytes.extend_from_slice(&cs.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 36]); // endpoints
        bytes.extend_from_slice(&[0u8; 12]); // gamma
        bytes.extend_from_slice(&0u32.to_le_bytes()); // intent
        let data_off = if icc.is_empty() { 0u32 } else { 124 };
        bytes.extend_from_slice(&data_off.to_le_bytes()); // bV5ProfileData
        bytes.extend_from_slice(&(icc.len() as u32).to_le_bytes()); // bV5ProfileSize
        bytes.extend_from_slice(&0u32.to_le_bytes()); // bV5Reserved
        bytes.extend_from_slice(icc);
        for row in (0..h as usize).rev() {
            bytes.extend_from_slice(&px[row * w as usize * 3..][..w as usize * 3]);
            bytes.extend_from_slice(&vec![0u8; stride - w as usize * 3]);
        }
        bytes
    }

    #[test]
    fn bmp_v5_color_space() {
        let px: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];

        let srgb_bytes = bmp_v5(&px, 2, 2, 0x7352_4742, &[]);
        let srgb = scan_bmp(&srgb_bytes).unwrap();
        assert!(matches!(srgb.color, SampleColor::Srgb));

        let icc = linear_profile();
        let embedded_bytes = bmp_v5(&px, 2, 2, 0x4d42_4544, &icc);
        let embedded = scan_bmp(&embedded_bytes).unwrap();
        assert!(
            matches!(&embedded.color, SampleColor::StoredIcc(b) if b.as_ref() == icc.as_slice())
        );

        let unreadable_bytes = bmp_v5(&px, 2, 2, 0, &[]);
        let unreadable = scan_bmp(&unreadable_bytes).unwrap();
        assert!(matches!(unreadable.color, SampleColor::Unsupported(_)));
    }

    #[test]
    fn bmp_v5_icc() {
        let px: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];
        let srgb_bytes = bmp_v5(&px, 2, 2, 0x7352_4742, &[]);
        let embedded_bytes = bmp_v5(&px, 2, 2, 0x4d42_4544, &linear_profile());
        let unreadable_bytes = bmp_v5(&px, 2, 2, 0, &[]);
        let resized = |src: &[u8]| {
            render(
                src,
                &recipe(src, 2, 2, OutputFormat::Png, None),
                &NO_CANCELLATION,
            )
        };
        assert_eq!(rgba(&resized(&srgb_bytes).unwrap()), rgba(&srgb_bytes));
        assert!((185..=190).contains(&rgba(&resized(&embedded_bytes).unwrap())[12]));
        assert!(resized(&unreadable_bytes).is_err());
    }
}
