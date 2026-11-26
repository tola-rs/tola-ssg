use std::borrow::Cow;

use crate::{ImageError, ImageResult};

use super::super::{ContainerMetadata, SampleColor};
use super::le16;
use crate::cancellation::{Cancellation, ensure_active_if_present};
use crate::check_dimensions;

/// How a damaged GIF reads to a site author.
const DAMAGED: &str = "the GIF image is damaged or incomplete; re-export it";

fn gif_sub_blocks(
    bytes: &[u8],
    off: &mut usize,
    mut out: Option<&mut Vec<u8>>,
    cancel: Option<&dyn Cancellation>,
) -> ImageResult<()> {
    loop {
        ensure_active_if_present(cancel)?;
        let len = usize::from(
            *bytes
                .get(*off)
                .ok_or_else(|| ImageError::unreadable(DAMAGED))?,
        );
        *off += 1;
        if len == 0 {
            return Ok(());
        }
        let block = bytes
            .get(*off..*off + len)
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        if let Some(buf) = out.as_deref_mut() {
            buf.extend_from_slice(block);
        }
        *off += len;
    }
}

pub(in crate::codec) fn scan_gif<'a>(
    bytes: &'a [u8],
    cancel: Option<&dyn Cancellation>,
) -> ImageResult<ContainerMetadata<'a>> {
    if bytes.len() < 13 {
        return Err(ImageError::unreadable(DAMAGED));
    }
    let canvas_w = u32::from(le16(&bytes[6..8]));
    let canvas_h = u32::from(le16(&bytes[8..10]));
    check_dimensions(canvas_w, canvas_h)?;
    let mut meta = ContainerMetadata::default();
    let mut off = 13
        + if bytes[10] & 128 != 0 {
            3usize << ((bytes[10] & 7) + 1)
        } else {
            0
        };
    let mut frames = 0usize;
    let mut alpha = false;
    let mut icc = None;
    loop {
        ensure_active_if_present(cancel)?;
        let marker = *bytes
            .get(off)
            .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
        off += 1;
        match marker {
            0x3b => {
                if frames == 0 || off != bytes.len() {
                    return Err(ImageError::unreadable(DAMAGED));
                }
                break;
            }
            0x2c => {
                let desc = bytes
                    .get(off..off + 9)
                    .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                let w = u32::from(le16(&desc[4..6]));
                let h = u32::from(le16(&desc[6..8]));
                check_dimensions(w, h)?;
                if u32::from(le16(desc)) + w > canvas_w
                    || u32::from(le16(&desc[2..4])) + h > canvas_h
                {
                    return Err(ImageError::unreadable(DAMAGED));
                }
                // The decoder leaves uncovered canvas transparent, ignoring the palette background.
                alpha |= w != canvas_w || h != canvas_h;
                off += 9 + if desc[8] & 128 != 0 {
                    3usize << ((desc[8] & 7) + 1)
                } else {
                    0
                };
                if off >= bytes.len() {
                    return Err(ImageError::unreadable(DAMAGED));
                }
                off += 1;
                gif_sub_blocks(bytes, &mut off, None, cancel)?;
                frames += 1;
                meta.is_animated |= frames > 1;
            }
            0x21 => {
                let label = *bytes
                    .get(off)
                    .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                off += 1;
                if label == 0xf9 {
                    let ctrl = bytes
                        .get(off..off + 6)
                        .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                    if ctrl[0] != 4 || ctrl[5] != 0 {
                        return Err(ImageError::unreadable(DAMAGED));
                    }
                    alpha |= ctrl[1] & 1 != 0;
                    off += 6;
                } else if label == 0xff {
                    let size = usize::from(
                        *bytes
                            .get(off)
                            .ok_or_else(|| ImageError::unreadable(DAMAGED))?,
                    );
                    off += 1;
                    let app = bytes
                        .get(off..off + size)
                        .ok_or_else(|| ImageError::unreadable(DAMAGED))?;
                    off += size;
                    if app == b"ICCRGBG1012" {
                        if icc.is_some() {
                            return Err(ImageError::unreadable(
                                "the GIF image has more than one color profile; re-export it with sRGB colors",
                            ));
                        }
                        let mut buf = Vec::new();
                        gif_sub_blocks(bytes, &mut off, Some(&mut buf), cancel)?;
                        icc = Some(buf);
                    } else {
                        gif_sub_blocks(bytes, &mut off, None, cancel)?;
                    }
                } else {
                    gif_sub_blocks(bytes, &mut off, None, cancel)?;
                }
            }
            _ => return Err(ImageError::unreadable(DAMAGED)),
        }
    }
    meta.has_alpha = Some(alpha);
    // The gif reader collects an ICC application extension only up to the first frame, so the
    // profile a GIF records anywhere in the file is this scan's to read.
    meta.color = match icc {
        Some(icc) => SampleColor::StoredIcc(Cow::Owned(icc)),
        None => SampleColor::Srgb,
    };
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use crate::NO_CANCELLATION;
    use crate::codec::tests::test_support::*;
    use crate::codec::{inspect, render};
    use crate::{
        ImageFormat, ImageRecipe, OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions,
    };

    const OPAQUE_GIF: &[u8] = &[
        b'G', b'I', b'F', b'8', b'9', b'a', 1, 0, 1, 0, 0x80, 0, 0, 255, 255, 255, 0, 0, 0, 0x2c,
        0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3b,
    ];

    #[test]
    fn gif_opaque_jpeg() {
        for ctrl in [None, Some([0x21, 0xf9, 4, 0, 0, 0, 1, 0])] {
            let mut src = OPAQUE_GIF.to_vec();
            if let Some(ctrl) = ctrl {
                src.splice(19..19, ctrl);
            }
            assert!(!inspect(&src).unwrap().has_alpha);
            let jpeg = render(
                &src,
                &recipe(&src, 1, 1, OutputFormat::Jpeg, None),
                &NO_CANCELLATION,
            )
            .unwrap();
            assert_eq!(inspect(&jpeg).unwrap().format, ImageFormat::Jpeg);
            assert_eq!(rgba(&jpeg), [255, 255, 255, 255]);
        }
    }

    #[test]
    fn gif_alpha_rejects_jpeg() {
        for (idx, alpha) in [(0, 0), (1, 255)] {
            let mut src = OPAQUE_GIF.to_vec();
            src.splice(19..19, [0x21, 0xf9, 4, 1, 0, 0, idx, 0]);
            let meta = inspect(&src).unwrap();
            assert!(meta.has_alpha);
            let png = render_png(&src, 1, 1);
            assert_eq!(rgba(&png)[3], alpha);
            assert!(
                ImageRecipe::resolve(
                    &meta,
                    ResizeOptions {
                        width: Some(1),
                        height: Some(1),
                        operation: ResizeOperation::Scale,
                        format: OutputFormat::Jpeg,
                        filter: ResizeFilter::Triangle,
                        ..ResizeOptions::default()
                    },
                )
                .is_err()
            );
        }
    }

    #[test]
    fn gif_partial_canvas_alpha() {
        for (w, h, left, top, expected) in [
            (2, 1, 1, 0, [0, 0, 0, 0, 255, 255, 255, 255]),
            (1, 2, 0, 1, [0, 0, 0, 0, 255, 255, 255, 255]),
        ] {
            let mut src = OPAQUE_GIF.to_vec();
            src[6] = w;
            src[8] = h;
            src[20] = left;
            src[22] = top;
            assert!(inspect(&src).unwrap().has_alpha);
            let png = render(
                &src,
                &recipe(&src, u32::from(w), u32::from(h), OutputFormat::Png, None),
                &NO_CANCELLATION,
            )
            .unwrap();
            assert_eq!(rgba(&png), expected);
        }
    }

    #[test]
    fn reject_animated_gif() {
        let out = render(
            OPAQUE_GIF,
            &recipe(OPAQUE_GIF, 1, 1, OutputFormat::Png, None),
            &NO_CANCELLATION,
        )
        .unwrap();
        assert_eq!(rgba(&out), [255, 255, 255, 255]);
        let mut anim = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut anim);
            for px in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let frame =
                    image::Frame::new(image::RgbaImage::from_raw(1, 1, px.to_vec()).unwrap());
                enc.encode_frame(frame).unwrap();
            }
        }
        let r = recipe(&anim, 1, 1, OutputFormat::Png, None);
        assert!(render(&anim, &r, &NO_CANCELLATION).is_err());
    }
}
