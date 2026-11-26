use crate::ImageResult;

use super::super::{ContainerMetadata, SampleColor};
use super::be32;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;
use crate::{ImageError, ImageFormat, ReexportProblem};

/// How a damaged PNG reads to a site author.
const DAMAGED: &str = "the PNG image is damaged or incomplete; re-export it";

const DAMAGED_COLOR: &str =
    "the PNG image has damaged color information; re-export it with sRGB colors";

/// These scans find animation and color metadata the codec wrappers hide; compressed
/// pixels are always decoded and encoded by maintained codec libraries.
pub(in crate::codec) fn scan_png<'a>(
    bytes: &'a [u8],
    cancel: Option<&dyn Cancellation>,
) -> ImageResult<ContainerMetadata<'a>> {
    let mut meta = ContainerMetadata::default();
    let mut off = 8usize;
    let mut cicp = None;
    let mut gamma = None;
    let mut srgb = false;
    let mut seen_icc = false;
    let mut std_chrm = true;
    let mut seen_chrm = false;
    let mut mastering = false;
    let mut seen_pixels = false;
    let mut ended = false;
    while off < bytes.len() {
        ensure_active_if_present(cancel)?;
        let hdr = bytes
            .get(off..off + 8)
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        if !hdr[4..8].iter().all(u8::is_ascii_alphabetic) || !hdr[6].is_ascii_uppercase() {
            return Err(ImageError::unreadable(DAMAGED));
        }
        let len = be32(hdr) as usize;
        let end = off
            .checked_add(12)
            .and_then(|start| start.checked_add(len))
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        if end > bytes.len() {
            return Err(ImageError::unreadable(DAMAGED));
        }
        let data = &bytes[off + 8..end - 4];
        // Skip IDAT/fdAT (decoder checks them); metadata after IDAT still needs CRC verification.
        if !matches!(&hdr[4..8], b"IDAT" | b"fdAT") {
            let mut crc = crc32fast::Hasher::new();
            for block in bytes[off + 4..end - 4].chunks(65_536) {
                ensure_active_if_present(cancel)?;
                crc.update(block);
            }
            if crc.finalize() != be32(&bytes[end - 4..end]) {
                return Err(ImageError::unreadable(DAMAGED));
            }
        }
        if seen_pixels
            && matches!(
                &hdr[4..8],
                b"PLTE" | b"iCCP" | b"sRGB" | b"gAMA" | b"cHRM" | b"cICP"
            )
        {
            return Err(ImageError::unreadable(DAMAGED));
        }
        match &hdr[4..8] {
            b"IHDR" => {
                if off != 8 || data.len() != 13 {
                    return Err(ImageError::unreadable(DAMAGED));
                }
                check_dimensions(be32(data), be32(&data[4..]))?;
            }
            b"acTL" | b"fcTL" | b"fdAT" => meta.is_animated = true,
            b"IDAT" => seen_pixels = true,
            b"iCCP" => {
                if seen_icc {
                    return Err(ImageError::unreadable(
                        "the PNG image has more than one color profile; re-export it with sRGB colors",
                    ));
                }
                seen_icc = true;
            }
            b"eXIf" => {
                if meta.exif.is_some() {
                    return Err(ImageError::unreadable(
                        "the PNG image has more than one EXIF block; re-export it",
                    ));
                }
                meta.exif = Some(data);
            }
            b"sRGB" => {
                if srgb || data.len() != 1 || data[0] > 3 {
                    return Err(ImageError::unreadable(DAMAGED_COLOR));
                }
                srgb = true;
            }
            b"gAMA" => {
                if gamma.is_some() || data.len() != 4 || be32(data) == 0 {
                    return Err(ImageError::unreadable(DAMAGED_COLOR));
                }
                gamma = Some(be32(data));
            }
            b"cHRM" => {
                if seen_chrm || data.len() != 32 {
                    return Err(ImageError::unreadable(DAMAGED_COLOR));
                }
                seen_chrm = true;
                let expected = [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
                std_chrm = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(expected)
                    .all(|(v, exp)| be32(v) == exp);
            }
            b"cICP" => {
                if cicp.is_some() || data.len() != 4 {
                    return Err(ImageError::unreadable(DAMAGED_COLOR));
                }
                cicp = Some([data[0], data[1], data[2], data[3]]);
            }
            b"mDCV" | b"cLLI" => mastering = true,
            b"IEND" => {
                if len != 0 || end != bytes.len() {
                    return Err(ImageError::unreadable(
                        "the PNG image has extra data after its end; re-export it",
                    ));
                }
                ended = true;
                break;
            }
            b"PLTE" => {}
            chunk if chunk[0].is_ascii_uppercase() => {
                return Err(ImageError::Reexport {
                    format: ImageFormat::Png,
                    problem: ReexportProblem::UnsupportedFeature,
                });
            }
            _ => {}
        }
        off = end;
    }
    if !ended {
        return Err(ImageError::unreadable(DAMAGED));
    }
    // Precedence: mastering -> cICP -> iCCP -> sRGB -> cHRM/gAMA.
    meta.color = if mastering {
        SampleColor::Unsupported(
            "the PNG image has HDR mastering data; convert the image to SDR sRGB",
        )
    } else if let Some(cicp) = cicp {
        if cicp == [1, 13, 0, 1] {
            SampleColor::Srgb
        } else {
            SampleColor::Unsupported(
                "the PNG image uses HDR color Tola cannot resize; convert the image to SDR sRGB",
            )
        }
    } else if seen_icc {
        // The iCCP profile needs inflating, which only the pixel decoder does.
        SampleColor::DecoderIcc
    } else if srgb {
        SampleColor::Srgb
    } else if !std_chrm {
        SampleColor::Unsupported(
            "the PNG image has non-sRGB colors but no color profile; re-export it with sRGB colors",
        )
    } else if let Some(gamma) = gamma {
        // gAMA is reciprocal encoding gamma; 45455 is the conventional sRGB value.
        if gamma == 45_455 {
            SampleColor::Srgb
        } else {
            SampleColor::Gamma(100_000.0 / gamma as f32)
        }
    } else {
        SampleColor::Srgb
    };
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::OutputFormat;
    use crate::codec::render;
    use crate::codec::tests::test_support::*;
    use image::ExtendedColorType;

    fn png_with(kind: &[u8; 4], data: &[u8], before_pixels: bool) -> Vec<u8> {
        let hdr = png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]);
        let px = png_chunk(b"IDAT", b"not compressed");
        let extra = png_chunk(kind, data);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&hdr);
        if before_pixels {
            bytes.extend_from_slice(&extra);
            bytes.extend_from_slice(&px);
        } else {
            bytes.extend_from_slice(&px);
            bytes.extend_from_slice(&extra);
        }
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        bytes
    }

    #[test]
    fn reject_late_color_chunks() {
        let gamma = 45_455u32.to_be_bytes();
        assert!(scan_png(&png_with(b"gAMA", &gamma, true), None).is_ok());
        assert!(scan_png(&png_with(b"gAMA", &gamma, false), None).is_err());
    }

    #[test]
    fn png_color_precedence() {
        let chrm_bytes: Vec<u8> = [31270u32, 32900, 68000, 32000, 26500, 69000, 15000, 6000]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let cicp = || (b"cICP", vec![1, 13, 0, 1]);
        let gama = || (b"gAMA", 100_000u32.to_be_bytes().to_vec());
        let chrm = || (b"cHRM", chrm_bytes.clone());
        let srgb = || (b"sRGB", vec![0]);
        for (icc, chunks) in [
            (None, vec![cicp(), gama()]),
            (None, vec![cicp(), chrm()]),
            (None, vec![cicp(), gama(), chrm()]),
            (Some(linear_profile()), vec![cicp(), gama()]),
            (Some(b"not an ICC profile".to_vec()), vec![cicp(), gama()]),
            (None, vec![srgb(), gama()]),
        ] {
            let mut src = png(&[128, 128, 128], 1, 1, ExtendedColorType::Rgb8, icc, None);
            for (kind, data) in chunks {
                insert_png_chunk(&mut src, kind, &data);
            }
            let out = render_png(&src, 1, 1);
            assert_eq!(rgba(&out)[..3], [128, 128, 128]);
        }
    }

    #[test]
    fn icc_overrides_srgb_gamma() {
        for (kind, data) in [
            (b"gAMA", 45_455u32.to_be_bytes().to_vec()),
            (b"sRGB", vec![0]),
        ] {
            let mut src = png(
                &[128, 128, 128],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(linear_profile()),
                None,
            );
            insert_png_chunk(&mut src, kind, &data);
            let out = render_png(&src, 1, 1);
            let px = rgba(&out);
            assert!(
                (185..=190).contains(&px[0]),
                "the embedded profile converts the samples, not {}: {px:?}",
                String::from_utf8_lossy(kind)
            );
        }
    }

    #[test]
    fn reject_unsupported_color() {
        for (kind, data) in [
            (b"cICP", vec![9, 16, 0, 1]),
            (b"mDCV", vec![0; 24]),
            (b"cLLI", vec![0; 8]),
        ] {
            let mut src = png(
                &[128, 128, 128],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(linear_profile()),
                None,
            );
            insert_png_chunk(&mut src, kind, &data);
            assert!(
                render(
                    &src,
                    &recipe(&src, 1, 1, OutputFormat::Png, None),
                    &NO_CANCELLATION
                )
                .is_err()
            );
        }
    }
}
