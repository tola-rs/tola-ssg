use crate::{ImageError, ImageResult};

use crate::{ImageFormat, ImageMetadata, check_dimensions};

pub(in crate::codec) fn looks_like_xml(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'<')
}

pub(in crate::codec) fn inspect_svg(bytes: &[u8]) -> ImageResult<ImageMetadata> {
    let xml = std::str::from_utf8(bytes).map_err(|error| {
        ImageError::codec("the SVG must be UTF-8 text; save the file as UTF-8", error)
    })?;
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| ImageError::codec("the SVG is not valid XML; fix the markup", error))?;
    let root = document.root_element();
    if root.tag_name().name() != "svg"
        || root
            .tag_name()
            .namespace()
            .is_some_and(|namespace| namespace != "http://www.w3.org/2000/svg")
    {
        return Err(ImageError::unreadable(
            "the file is XML but not an SVG image; add a root `<svg>` element",
        ));
    }
    let view_box = root
        .attribute("viewBox")
        .map(|value| -> ImageResult<(f64, f64)> {
            let mut numbers = value
                .split(|character: char| character.is_ascii_whitespace() || character == ',')
                .filter(|part| !part.is_empty());
            let mut values = [0.0f64; 4];
            for value in &mut values {
                *value = numbers
                    .next()
                    .ok_or_else(|| {
                        ImageError::unreadable("the `viewBox` attribute needs four numbers")
                    })?
                    .parse()
                    .map_err(|error| {
                        ImageError::codec(
                            "the `viewBox` attribute contains a value that is not a number",
                            error,
                        )
                    })?;
            }
            if numbers.next().is_some()
                || values.iter().any(|value| !value.is_finite())
                || values[2] <= 0.0
                || values[3] <= 0.0
            {
                return Err(ImageError::unreadable(
                    "the `viewBox` attribute needs four numbers with positive width and height",
                ));
            }
            Ok((values[2], values[3]))
        })
        .transpose()?;
    let width = root
        .attribute("width")
        .map(svg_length)
        .transpose()?
        .flatten();
    let height = root
        .attribute("height")
        .map(svg_length)
        .transpose()?
        .flatten();
    let (width, height) = match (width, height, view_box) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some((vw, vh))) => (width, width * vh / vw),
        (None, Some(height), Some((vw, vh))) => (height * vw / vh, height),
        (None, None, Some(size)) => size,
        _ => {
            return Err(ImageError::unreadable(
                "the root `<svg>` element needs `width` and `height` or a `viewBox`",
            ));
        }
    };
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width.ceil() > f64::from(u32::MAX)
        || height.ceil() > f64::from(u32::MAX)
    {
        return Err(ImageError::unreadable(
            "the SVG `width` and `height` are too large; set smaller values",
        ));
    }
    let (width, height) = (width.ceil() as u32, height.ceil() as u32);
    check_dimensions(width, height)?;
    Ok(ImageMetadata {
        width,
        height,
        format: ImageFormat::Svg,
        has_alpha: true,
        is_lossy: false,
    })
}

fn svg_length(value: &str) -> ImageResult<Option<f64>> {
    let value = value.trim();
    if value.ends_with('%') || value == "auto" {
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
    let (number, factor) = units
        .iter()
        .find_map(|(unit, factor)| value.strip_suffix(unit).map(|number| (number, *factor)))
        .unwrap_or((value, 1.0));
    let number: f64 = number.trim().parse().map_err(|error| ImageError::codec("the SVG `width` and `height` must be numbers with units such as `px`, or use a `viewBox`", error))?;
    let pixels = number * factor;
    if !pixels.is_finite() || pixels <= 0.0 {
        return Err(ImageError::unreadable(
            "the SVG `width` and `height` must be positive numbers",
        ));
    }
    Ok(Some(pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::inspect;

    #[test]
    fn svg_dimensions_resolve_absolute_units() {
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1in" viewBox="0 0 4 2"/>"#;
        let metadata = inspect(source).unwrap();
        assert_eq!(
            (metadata.width, metadata.height, metadata.format),
            (96, 48, ImageFormat::Svg)
        );
        assert!(inspect(br#"<svg viewBox="0 0 NaN 20"/>"#).is_err());
    }
}
