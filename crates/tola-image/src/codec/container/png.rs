use anyhow::{Context, Result, bail, ensure};

use super::super::{ContainerMetadata, SampleColor};
use super::be32;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;

/// These scans find animation and color metadata the codec wrappers hide; compressed
/// pixels are always decoded and encoded by maintained codec libraries.
pub(in crate::codec) fn scan_png<'a>(
    bytes: &'a [u8],
    cancellation: Option<&dyn Cancellation>,
) -> Result<ContainerMetadata<'a>> {
    let mut metadata = ContainerMetadata::default();
    let mut offset = 8usize;
    let mut cicp = None;
    let mut gamma = None;
    let mut srgb = false;
    let mut seen_profile = false;
    let mut standard_chromaticities = true;
    let mut seen_chromaticities = false;
    let mut mastering = false;
    let mut seen_pixels = false;
    let mut ended = false;
    while offset < bytes.len() {
        ensure_active_if_present(cancellation)?;
        let header = bytes
            .get(offset..offset + 8)
            .context("the PNG image is damaged or incomplete; re-export it")?;
        ensure!(
            header[4..8].iter().all(u8::is_ascii_alphabetic) && header[6].is_ascii_uppercase(),
            "the PNG image is damaged or incomplete; re-export it"
        );
        let length = be32(header) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|start| start.checked_add(length))
            .context("the PNG image is damaged or incomplete; re-export it")?;
        ensure!(
            end <= bytes.len(),
            "the PNG image is damaged or incomplete; re-export it"
        );
        let data = &bytes[offset + 8..end - 4];
        // Inspect metadata without inflating image data. The pixel decoder checks IDAT itself;
        // metadata after IDAT still needs its own integrity check before orientation is trusted.
        if !matches!(&header[4..8], b"IDAT" | b"fdAT") {
            let mut checksum = crc32fast::Hasher::new();
            for block in bytes[offset + 4..end - 4].chunks(65_536) {
                ensure_active_if_present(cancellation)?;
                checksum.update(block);
            }
            ensure!(
                checksum.finalize() == be32(&bytes[end - 4..end]),
                "the PNG image is damaged or incomplete; re-export it"
            );
        }
        if matches!(
            &header[4..8],
            b"PLTE" | b"iCCP" | b"sRGB" | b"gAMA" | b"cHRM" | b"cICP"
        ) {
            ensure!(
                !seen_pixels,
                "the PNG image is damaged or incomplete; re-export it"
            );
        }
        match &header[4..8] {
            b"IHDR" => {
                ensure!(
                    offset == 8 && data.len() == 13,
                    "the PNG image is damaged or incomplete; re-export it"
                );
                check_dimensions(be32(data), be32(&data[4..]))?;
            }
            b"acTL" | b"fcTL" | b"fdAT" => metadata.is_animated = true,
            b"IDAT" => seen_pixels = true,
            b"iCCP" => {
                ensure!(
                    !seen_profile,
                    "the PNG image has more than one color profile; re-export it with sRGB colors"
                );
                seen_profile = true;
            }
            b"eXIf" => {
                ensure!(
                    metadata.exif.is_none(),
                    "the PNG image has more than one EXIF block; re-export it"
                );
                metadata.exif = Some(data);
            }
            b"sRGB" => {
                ensure!(
                    !srgb && data.len() == 1 && data[0] <= 3,
                    "the PNG image has damaged color information; re-export it with sRGB colors"
                );
                srgb = true;
            }
            b"gAMA" => {
                ensure!(
                    gamma.is_none() && data.len() == 4 && be32(data) > 0,
                    "the PNG image has damaged color information; re-export it with sRGB colors"
                );
                gamma = Some(be32(data));
            }
            b"cHRM" => {
                ensure!(
                    !seen_chromaticities && data.len() == 32,
                    "the PNG image has damaged color information; re-export it with sRGB colors"
                );
                seen_chromaticities = true;
                let expected = [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
                standard_chromaticities = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(expected)
                    .all(|(value, expected)| be32(value) == expected);
            }
            b"cICP" => {
                ensure!(
                    cicp.is_none() && data.len() == 4,
                    "the PNG image has damaged color information; re-export it with sRGB colors"
                );
                cicp = Some([data[0], data[1], data[2], data[3]]);
            }
            b"mDCV" | b"cLLI" => mastering = true,
            b"IEND" => {
                ensure!(
                    length == 0 && end == bytes.len(),
                    "the PNG image has extra data after its end; re-export it"
                );
                ended = true;
                break;
            }
            b"PLTE" => {}
            chunk if chunk[0].is_ascii_uppercase() => {
                bail!("the PNG image uses a feature Tola cannot read; re-export it")
            }
            _ => {}
        }
        offset = end;
    }
    ensure!(
        ended,
        "the PNG image is damaged or incomplete; re-export it"
    );
    // Colour chunks are read by the precedence the format defines — cICP, iCCP, sRGB, then cHRM
    // with gAMA — while every one of them was still checked above for the place, count, and shape
    // the format requires. HDR mastering data outranks the colour chunks it accompanies.
    metadata.color = if mastering {
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
    } else if seen_profile {
        // The iCCP profile needs inflating, which only the pixel decoder does.
        SampleColor::DecoderIcc
    } else if srgb {
        SampleColor::Srgb
    } else if !standard_chromaticities {
        SampleColor::Unsupported(
            "the PNG image has non-sRGB colors but no color profile; re-export it with sRGB colors",
        )
    } else if let Some(gamma) = gamma {
        // gAMA is the reciprocal encoding exponent, not the decoding exponent expected by ICC.
        // The conventional rounded sRGB gamma is interpreted as sRGB, not a simple power curve.
        if gamma == 45_455 {
            SampleColor::Srgb
        } else {
            SampleColor::Gamma(100_000.0 / gamma as f32)
        }
    } else {
        SampleColor::Srgb
    };
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputFormat;
    use crate::codec::render;
    use crate::codec::test_support::*;
    use image::ExtendedColorType;

    /// One 8x8 PNG whose pixels are not compressed — the scan never inflates them — with one
    /// chunk placed either side of the pixel data.
    fn png_with(kind: &[u8; 4], data: &[u8], before_pixels: bool) -> Vec<u8> {
        let header = png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 2, 0, 0, 0]);
        let pixels = png_chunk(b"IDAT", b"not compressed");
        let extra = png_chunk(kind, data);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&header);
        if before_pixels {
            bytes.extend_from_slice(&extra);
            bytes.extend_from_slice(&pixels);
        } else {
            bytes.extend_from_slice(&pixels);
            bytes.extend_from_slice(&extra);
        }
        bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
        bytes
    }

    /// Colour metadata has to be read before the pixels it describes. The same chunk after the
    /// pixel data is refused, and before it is accepted, so this cannot pass by refusing both.
    #[test]
    fn colour_metadata_after_pixels_is_refused() {
        let gamma = 45_455u32.to_be_bytes();
        assert!(scan_png(&png_with(b"gAMA", &gamma, true), None).is_ok());
        assert!(scan_png(&png_with(b"gAMA", &gamma, false), None).is_err());
    }

    /// The declaration a PNG's own precedence selects decides the samples, and every chunk below
    /// it is inert: cICP and sRGB name sRGB samples, so no curve, primaries, or profile below
    /// them converts anything — and a profile below cICP is not a profile to refuse either.
    #[test]
    fn colour_chunk_precedence_keeps_srgb_samples() {
        let chromaticities: Vec<u8> = [31270u32, 32900, 68000, 32000, 26500, 69000, 15000, 6000]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let cicp = || (b"cICP", vec![1, 13, 0, 1]);
        let gama = || (b"gAMA", 100_000u32.to_be_bytes().to_vec());
        let chrm = || (b"cHRM", chromaticities.clone());
        let srgb = || (b"sRGB", vec![0]);
        for (profile, chunks) in [
            (None, vec![cicp(), gama()]),
            (None, vec![cicp(), chrm()]),
            (None, vec![cicp(), gama(), chrm()]),
            (Some(linear_profile()), vec![cicp(), gama()]),
            (Some(b"not an ICC profile".to_vec()), vec![cicp(), gama()]),
            (None, vec![srgb(), gama()]),
        ] {
            let mut source = png(
                &[128, 128, 128],
                1,
                1,
                ExtendedColorType::Rgb8,
                profile,
                None,
            );
            for (kind, data) in chunks {
                insert_png_chunk(&mut source, kind, &data);
            }
            let output = render(
                &source,
                &recipe(&source, 1, 1, OutputFormat::Png, None),
                &active,
            )
            .unwrap();
            assert_eq!(rgba(&output)[..3], [128, 128, 128]);
        }
    }

    /// A profile outranks the declarations below it: picking the gAMA that names the sRGB gamma,
    /// or the sRGB chunk itself, leaves the samples on 128 where the profile converts them.
    #[test]
    fn icc_overrides_srgb_and_gamma() {
        for (kind, data) in [
            (b"gAMA", 45_455u32.to_be_bytes().to_vec()),
            (b"sRGB", vec![0]),
        ] {
            let mut source = png(
                &[128, 128, 128],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(linear_profile()),
                None,
            );
            insert_png_chunk(&mut source, kind, &data);
            let output = render(
                &source,
                &recipe(&source, 1, 1, OutputFormat::Png, None),
                &active,
            )
            .unwrap();
            let pixel = rgba(&output);
            assert!(
                (185..=190).contains(&pixel[0]),
                "the embedded profile converts the samples, not {}: {pixel:?}",
                String::from_utf8_lossy(kind)
            );
        }
    }

    /// A declaration Tola cannot resize is refused even when the profile below it could convert
    /// the samples.
    #[test]
    fn unsupported_colour_is_refused() {
        for (kind, data) in [
            (b"cICP", vec![9, 16, 0, 1]),
            (b"mDCV", vec![0; 24]),
            (b"cLLI", vec![0; 8]),
        ] {
            let mut source = png(
                &[128, 128, 128],
                1,
                1,
                ExtendedColorType::Rgb8,
                Some(linear_profile()),
                None,
            );
            insert_png_chunk(&mut source, kind, &data);
            assert!(
                render(
                    &source,
                    &recipe(&source, 1, 1, OutputFormat::Png, None),
                    &active
                )
                .is_err()
            );
        }
    }
}
