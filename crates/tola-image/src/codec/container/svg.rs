use crate::{ImageError, ImageResult};

use crate::{ImageFormat, ImageMetadata, check_dimensions};

pub(in crate::codec) fn looks_like_xml(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'<')
}

pub(in crate::codec) fn inspect_svg(bytes: &[u8]) -> ImageResult<ImageMetadata> {
    let xml = std::str::from_utf8(bytes).map_err(|err| {
        ImageError::codec("the SVG must be UTF-8 text; save the file as UTF-8", err)
    })?;
    let doc = roxmltree::Document::parse(xml)
        .map_err(|err| ImageError::codec("the SVG is not valid XML; fix the markup", err))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg"
        || root
            .tag_name()
            .namespace()
            .is_some_and(|ns| ns != "http://www.w3.org/2000/svg")
    {
        return Err(ImageError::unreadable(
            "the file is XML but not an SVG image; add a root `<svg>` element",
        ));
    }
    let view_box = root
        .attribute("viewBox")
        .map(|val| -> ImageResult<(f64, f64)> {
            let mut nums = val
                .split(|c: char| c.is_ascii_whitespace() || c == ',')
                .filter(|part| !part.is_empty());
            let mut vals = [0.0f64; 4];
            for v in &mut vals {
                *v = nums
                    .next()
                    .ok_or_else(|| {
                        ImageError::unreadable("the `viewBox` attribute needs four numbers")
                    })?
                    .parse()
                    .map_err(|err| {
                        ImageError::codec(
                            "the `viewBox` attribute contains a value that is not a number",
                            err,
                        )
                    })?;
            }
            if nums.next().is_some()
                || vals.iter().any(|v| !v.is_finite())
                || vals[2] <= 0.0
                || vals[3] <= 0.0
            {
                return Err(ImageError::unreadable(
                    "the `viewBox` attribute needs four numbers with positive width and height",
                ));
            }
            Ok((vals[2], vals[3]))
        })
        .transpose()?;
    let w = root
        .attribute("width")
        .map(svg_length)
        .transpose()?
        .flatten();
    let h = root
        .attribute("height")
        .map(svg_length)
        .transpose()?
        .flatten();
    let (w, h) = match (w, h, view_box) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((vw, vh))) => (w, w * vh / vw),
        (None, Some(h), Some((vw, vh))) => (h * vw / vh, h),
        (None, None, Some(size)) => size,
        _ => {
            return Err(ImageError::unreadable(
                "the root `<svg>` element needs `width` and `height` or a `viewBox`",
            ));
        }
    };
    if !w.is_finite()
        || !h.is_finite()
        || w <= 0.0
        || h <= 0.0
        || w.ceil() > f64::from(u32::MAX)
        || h.ceil() > f64::from(u32::MAX)
    {
        return Err(ImageError::unreadable(
            "the SVG `width` and `height` are too large; set smaller values",
        ));
    }
    let (w, h) = (w.ceil() as u32, h.ceil() as u32);
    check_dimensions(w, h)?;
    Ok(ImageMetadata {
        width: w,
        height: h,
        format: ImageFormat::Svg,
        has_alpha: true,
        is_lossy: false,
    })
}

fn svg_length(val: &str) -> ImageResult<Option<f64>> {
    let val = val.trim();
    if val.ends_with('%') || val == "auto" {
        return Ok(None);
    }
    let units = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("Q", 96.0 / 101.6),
    ];
    let (num, factor) = units
        .iter()
        .find_map(|(unit, factor)| val.strip_suffix(unit).map(|num| (num, *factor)))
        .unwrap_or((val, 1.0));
    let num: f64 = num.trim().parse().map_err(|err| {
        ImageError::codec(
            "the SVG `width` and `height` must be numbers with units such as `px`, or use a `viewBox`",
            err,
        )
    })?;
    let px = num * factor;
    if !px.is_finite() || px <= 0.0 {
        return Err(ImageError::unreadable(
            "the SVG `width` and `height` must be positive numbers",
        ));
    }
    Ok(Some(px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::inspect;

    #[test]
    fn svg_absolute_units() {
        let src = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1in" viewBox="0 0 4 2"/>"#;
        let meta = inspect(src).unwrap();
        assert_eq!(
            (meta.width, meta.height, meta.format),
            (96, 48, ImageFormat::Svg)
        );
        assert!(inspect(br#"<svg viewBox="0 0 NaN 20"/>"#).is_err());
    }
}
