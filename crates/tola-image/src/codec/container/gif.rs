use std::borrow::Cow;

use anyhow::{Context, Result, bail, ensure};

use super::super::{ContainerMetadata, SampleColor};
use super::le16;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;

fn gif_sub_blocks(
    bytes: &[u8],
    offset: &mut usize,
    mut collect: Option<&mut Vec<u8>>,
    cancellation: Option<&dyn Cancellation>,
) -> Result<()> {
    loop {
        ensure_active_if_present(cancellation)?;
        let length = usize::from(
            *bytes
                .get(*offset)
                .context("the GIF image is damaged or incomplete; re-export it")?,
        );
        *offset += 1;
        if length == 0 {
            return Ok(());
        }
        let block = bytes
            .get(*offset..*offset + length)
            .context("the GIF image is damaged or incomplete; re-export it")?;
        if let Some(output) = collect.as_deref_mut() {
            output.extend_from_slice(block);
        }
        *offset += length;
    }
}

pub(in crate::codec) fn scan_gif<'a>(
    bytes: &'a [u8],
    cancellation: Option<&dyn Cancellation>,
) -> Result<ContainerMetadata<'a>> {
    ensure!(
        bytes.len() >= 13,
        "the GIF image is damaged or incomplete; re-export it"
    );
    let canvas_width = u32::from(le16(&bytes[6..8]));
    let canvas_height = u32::from(le16(&bytes[8..10]));
    check_dimensions(canvas_width, canvas_height)?;
    let mut metadata = ContainerMetadata::default();
    let mut offset = 13
        + if bytes[10] & 128 != 0 {
            3usize << ((bytes[10] & 7) + 1)
        } else {
            0
        };
    let mut frames = 0usize;
    let mut has_alpha = false;
    let mut profile = None;
    loop {
        ensure_active_if_present(cancellation)?;
        let marker = *bytes
            .get(offset)
            .context("the GIF image is damaged or incomplete; re-export it")?;
        offset += 1;
        match marker {
            0x3b => {
                ensure!(
                    frames > 0 && offset == bytes.len(),
                    "the GIF image is damaged or incomplete; re-export it"
                );
                break;
            }
            0x2c => {
                let descriptor = bytes
                    .get(offset..offset + 9)
                    .context("the GIF image is damaged or incomplete; re-export it")?;
                let width = u32::from(le16(&descriptor[4..6]));
                let height = u32::from(le16(&descriptor[6..8]));
                check_dimensions(width, height)?;
                ensure!(
                    u32::from(le16(descriptor)) + width <= canvas_width
                        && u32::from(le16(&descriptor[2..4])) + height <= canvas_height,
                    "the GIF image is damaged or incomplete; re-export it"
                );
                // The decoder leaves uncovered canvas transparent, ignoring the palette background.
                has_alpha |= width != canvas_width || height != canvas_height;
                offset += 9 + if descriptor[8] & 128 != 0 {
                    3usize << ((descriptor[8] & 7) + 1)
                } else {
                    0
                };
                ensure!(
                    bytes.get(offset).is_some(),
                    "the GIF image is damaged or incomplete; re-export it"
                );
                offset += 1;
                gif_sub_blocks(bytes, &mut offset, None, cancellation)?;
                frames += 1;
                metadata.is_animated |= frames > 1;
            }
            0x21 => {
                let label = *bytes
                    .get(offset)
                    .context("the GIF image is damaged or incomplete; re-export it")?;
                offset += 1;
                if label == 0xf9 {
                    let control = bytes
                        .get(offset..offset + 6)
                        .context("the GIF image is damaged or incomplete; re-export it")?;
                    ensure!(
                        control[0] == 4 && control[5] == 0,
                        "the GIF image is damaged or incomplete; re-export it"
                    );
                    has_alpha |= control[1] & 1 != 0;
                    offset += 6;
                } else if label == 0xff {
                    let size = usize::from(
                        *bytes
                            .get(offset)
                            .context("the GIF image is damaged or incomplete; re-export it")?,
                    );
                    offset += 1;
                    let application = bytes
                        .get(offset..offset + size)
                        .context("the GIF image is damaged or incomplete; re-export it")?;
                    offset += size;
                    if application == b"ICCRGBG1012" {
                        ensure!(
                            profile.is_none(),
                            "the GIF image has more than one color profile; re-export it with sRGB colors"
                        );
                        let mut collected = Vec::new();
                        gif_sub_blocks(bytes, &mut offset, Some(&mut collected), cancellation)?;
                        profile = Some(collected);
                    } else {
                        gif_sub_blocks(bytes, &mut offset, None, cancellation)?;
                    }
                } else {
                    gif_sub_blocks(bytes, &mut offset, None, cancellation)?;
                }
            }
            _ => bail!("the GIF image is damaged or incomplete; re-export it"),
        }
    }
    metadata.has_alpha = Some(has_alpha);
    // The gif reader collects an ICC application extension only up to the first frame, so the
    // profile a GIF records anywhere in the file is this scan's to read.
    metadata.color = match profile {
        Some(profile) => SampleColor::StoredIcc(Cow::Owned(profile)),
        None => SampleColor::Srgb,
    };
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use crate::codec::test_support::*;
    use crate::codec::{inspect, render};
    use crate::{
        ImageFormat, ImageRecipe, OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions,
    };

    const OPAQUE_GIF: &[u8] = &[
        b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, 255, 255, 255, 0, 0, 0, 0x2c,
        0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3b,
    ];

    #[test]
    fn opaque_gif_allows_jpeg() {
        for control in [None, Some([0x21, 0xf9, 4, 0, 0, 0, 1, 0])] {
            let mut source = OPAQUE_GIF.to_vec();
            if let Some(control) = control {
                source.splice(19..19, control);
            }
            assert!(!inspect(&source).unwrap().has_alpha);
            let jpeg = render(
                &source,
                &recipe(&source, 1, 1, OutputFormat::Jpeg, None),
                &active,
            )
            .unwrap();
            assert_eq!(inspect(&jpeg).unwrap().format, ImageFormat::Jpeg);
            assert_eq!(rgba(&jpeg), [255, 255, 255, 255]);
        }
    }

    #[test]
    fn gif_transparency_guards_jpeg() {
        for (transparent_index, alpha) in [(0, 0), (1, 255)] {
            let mut source = OPAQUE_GIF.to_vec();
            source.splice(19..19, [0x21, 0xf9, 4, 1, 0, 0, transparent_index, 0]);
            let metadata = inspect(&source).unwrap();
            assert!(metadata.has_alpha);
            let png = render(
                &source,
                &recipe(&source, 1, 1, OutputFormat::Png, None),
                &active,
            )
            .unwrap();
            assert_eq!(rgba(&png)[3], alpha);
            assert!(
                ImageRecipe::resolve(
                    &metadata,
                    ResizeOptions {
                        width: Some(1),
                        height: Some(1),
                        operation: ResizeOperation::Scale,
                        format: OutputFormat::Jpeg,
                        quality: None,
                        filter: ResizeFilter::Triangle,
                        background: None,
                    },
                )
                .is_err()
            );
        }
    }

    #[test]
    fn partial_gif_canvas_keeps_transparency() {
        for (width, height, left, top, expected) in [
            (2, 1, 1, 0, [0, 0, 0, 0, 255, 255, 255, 255]),
            (1, 2, 0, 1, [0, 0, 0, 0, 255, 255, 255, 255]),
        ] {
            let mut source = OPAQUE_GIF.to_vec();
            source[6] = width;
            source[8] = height;
            source[20] = left;
            source[22] = top;
            assert!(inspect(&source).unwrap().has_alpha);
            let png = render(
                &source,
                &recipe(
                    &source,
                    u32::from(width),
                    u32::from(height),
                    OutputFormat::Png,
                    None,
                ),
                &active,
            )
            .unwrap();
            assert_eq!(rgba(&png), expected);
        }
    }

    #[test]
    fn animated_gif_input_is_rejected() {
        let output = render(
            OPAQUE_GIF,
            &recipe(OPAQUE_GIF, 1, 1, OutputFormat::Png, None),
            &active,
        )
        .unwrap();
        assert_eq!(rgba(&output), [255, 255, 255, 255]);
        let mut animation = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut animation);
            for pixels in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let frame =
                    image::Frame::new(image::RgbaImage::from_raw(1, 1, pixels.to_vec()).unwrap());
                encoder.encode_frame(frame).unwrap();
            }
        }
        let recipe = recipe(&animation, 1, 1, OutputFormat::Png, None);
        assert!(render(&animation, &recipe, &active).is_err());
    }
}
