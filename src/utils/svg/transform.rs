use anyhow::Result;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::BytesStart;
use crate::utils::svg::{SVG_PADDING_TOP, SVG_PADDING_BOTTOM};

/// Transform SVG attributes (height, viewBox adjustments).
pub fn transform_svg_attrs<'a>(elem: &'a BytesStart<'_>) -> Result<Vec<Attribute<'a>>> {
    let attrs = elem.attributes();
    // SVG typically has ~5-8 attributes
    let mut result = Vec::with_capacity(8);
    for attr in attrs.flatten() {
        let transformed = match attr.key.as_ref() {
            b"height" => adjust_height(attr)?,
            b"viewBox" => adjust_viewbox(attr)?,
            _ => attr,
        };
        result.push(transformed);
    }
    Ok(result)
}

/// Adjust height attribute (add top padding)
fn adjust_height(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let value = std::str::from_utf8(attr.value.as_ref())?;
    let height: f32 = value.trim_end_matches("pt").parse()?;

    Ok(Attribute {
        key: attr.key,
        value: format!("{}pt", height + SVG_PADDING_TOP)
            .into_bytes()
            .into(),
    })
}

/// Adjust viewBox attribute (expand for padding)
fn adjust_viewbox(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let value = std::str::from_utf8(attr.value.as_ref())?;

    // Parse 4 values without allocating a Vec
    let mut iter = value.split_whitespace();
    let (v0, v1, v2, v3) = match (iter.next(), iter.next(), iter.next(), iter.next()) {
        (Some(a), Some(b), Some(c), Some(d)) => (
            a.parse::<f32>().ok(),
            b.parse::<f32>().ok(),
            c.parse::<f32>().ok(),
            d.parse::<f32>().ok(),
        ),
        _ => anyhow::bail!("Invalid viewBox: expected 4 values"),
    };

    let (v0, v1, v2, v3) = match (v0, v1, v2, v3) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => anyhow::bail!("Invalid viewBox: failed to parse values"),
    };

    let new_viewbox = format!(
        "{} {} {} {}",
        v0,
        v1 - SVG_PADDING_TOP,
        v2,
        v3 + SVG_PADDING_TOP + SVG_PADDING_BOTTOM
    );

    Ok(Attribute {
        key: attr.key,
        value: new_viewbox.into_bytes().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::name::QName;

    #[test]
    fn test_adjust_viewbox_valid() {
        let attr = Attribute {
            key: QName(b"viewBox"),
            value: b"0 0 100 200".as_slice().into(),
        };
        let result = adjust_viewbox(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();

        // Check padding adjustments: y -= 5, height += 9
        assert!(value.contains("0 -5 100 209"));
    }

    #[test]
    fn test_adjust_viewbox_with_decimals() {
        let attr = Attribute {
            key: QName(b"viewBox"),
            value: b"0.5 1.5 100.5 200.5".as_slice().into(),
        };
        let result = adjust_viewbox(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert!(value.starts_with("0.5 -3.5")); // 1.5 - 5.0 = -3.5
    }

    #[test]
    fn test_adjust_viewbox_invalid() {
        // Not enough values
        let attr = Attribute {
            key: QName(b"viewBox"),
            value: b"0 0 100".as_slice().into(),
        };
        assert!(adjust_viewbox(attr).is_err());

        // Non-numeric values
        let attr = Attribute {
            key: QName(b"viewBox"),
            value: b"a b c d".as_slice().into(),
        };
        assert!(adjust_viewbox(attr).is_err());
    }

    #[test]
    fn test_adjust_height() {
        let attr = Attribute {
            key: QName(b"height"),
            value: b"100pt".as_slice().into(),
        };
        let result = adjust_height(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert_eq!(value, "105pt"); // 100 + 5 padding
    }

    #[test]
    fn test_adjust_height_decimal() {
        let attr = Attribute {
            key: QName(b"height"),
            value: b"50.5pt".as_slice().into(),
        };
        let result = adjust_height(attr).unwrap();
        let value = std::str::from_utf8(&result.value).unwrap();
        assert_eq!(value, "55.5pt");
    }

    #[test]
    fn test_transform_svg_attrs() {
        use quick_xml::Reader;
        use quick_xml::events::Event;

        let xml = r#"<svg width="100" height="100pt" viewBox="0 0 100 100">"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        if let Ok(Event::Start(ref e)) = reader.read_event() {
            let attrs = transform_svg_attrs(e).unwrap();

            // Check if attributes are transformed
            let mut found_height = false;
            let mut found_viewbox = false;

            for attr in attrs {
                match attr.key.as_ref() {
                    b"height" => {
                        found_height = true;
                        assert_eq!(attr.value.as_ref(), b"105pt"); // 100 + 5
                    }
                    b"viewBox" => {
                        found_viewbox = true;
                        // 0 0 100 100 -> 0 -5 100 109
                        assert_eq!(attr.value.as_ref(), b"0 -5 100 109");
                    }
                    b"width" => {
                        assert_eq!(attr.value.as_ref(), b"100");
                    }
                    _ => {}
                }
            }
            assert!(found_height);
            assert!(found_viewbox);
        } else {
            panic!("Failed to read start event");
        }
    }
}
