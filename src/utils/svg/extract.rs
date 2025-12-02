use anyhow::{Context, Result};
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use std::fmt::Write as FmtWrite;
use std::io::Cursor;

use crate::config::SiteConfig;
use crate::utils::meta::url_from_output_path;
use crate::utils::svg::{HtmlContext, Svg, INITIAL_SVG_BUFFER_SIZE};
use super::optimize::optimize_svg;
use super::transform::transform_svg_attrs;

/// Extract and optimize an SVG element, writing an img placeholder to the output
pub fn extract_svg_element(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: &BytesStart<'_>,
    ctx: &mut HtmlContext<'_>,
) -> Result<Option<Svg>> {
    // Process SVG data (transform, capture, optimize)
    let (optimized_data, size) = process_svg_data(reader, elem, ctx.config)?;

    // Create SVG and write placeholder
    let svg = Svg::new(optimized_data, size, ctx.svg_count);
    ctx.svg_count += 1;

    write_img_placeholder(writer, &svg, ctx)?;

    Ok(Some(svg))
}

/// Process SVG element: transform attributes, capture content, and optimize.
fn process_svg_data(
    reader: &mut Reader<&[u8]>,
    elem: &BytesStart<'_>,
    config: &SiteConfig,
) -> Result<(Vec<u8>, (f32, f32))> {
    // Transform SVG attributes (adjust height/viewBox for typst quirks)
    let attrs = transform_svg_attrs(elem)?;

    // Capture complete SVG content
    let raw_svg = capture_svg_content(reader, &attrs)?;

    // Optimize with usvg
    optimize_svg(&raw_svg, config)
}

/// Capture complete SVG element content from reader.
fn capture_svg_content(reader: &mut Reader<&[u8]>, attrs: &[Attribute<'_>]) -> Result<Vec<u8>> {
    let mut content = Vec::with_capacity(INITIAL_SVG_BUFFER_SIZE);

    // Write opening tag manually
    content.extend_from_slice(b"<svg");
    for attr in attrs {
        content.push(b' ');
        content.extend_from_slice(attr.key.as_ref());
        content.extend_from_slice(b"=\"");
        content.extend_from_slice(attr.value.as_ref());
        content.push(b'"');
    }
    content.push(b'>');

    // Capture nested content by tracking depth
    let mut depth = 1u32;
    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(e) => {
                depth += 1;
                content.push(b'<');
                content.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    content.push(b' ');
                    content.extend_from_slice(attr.key.as_ref());
                    content.extend_from_slice(b"=\"");
                    content.extend_from_slice(attr.value.as_ref());
                    content.push(b'"');
                }
                content.push(b'>');
            }
            Event::End(e) => {
                if e.name().as_ref() == b"svg" {
                    depth -= 1;
                    if depth == 0 {
                        content.extend_from_slice(b"</svg>");
                        break;
                    }
                } else {
                    depth -= 1;
                }
                content.extend_from_slice(b"</");
                content.extend_from_slice(e.name().as_ref());
                content.push(b'>');
            }
            Event::Empty(e) => {
                content.push(b'<');
                content.extend_from_slice(e.name().as_ref());
                for attr in e.attributes() {
                    let attr = attr?;
                    content.push(b' ');
                    content.extend_from_slice(attr.key.as_ref());
                    content.extend_from_slice(b"=\"");
                    content.extend_from_slice(attr.value.as_ref());
                    content.push(b'"');
                }
                content.extend_from_slice(b"/>");
            }
            Event::Text(e) => content.extend_from_slice(e.as_ref()),
            Event::CData(e) => {
                content.extend_from_slice(b"<![CDATA[");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"]]>");
            }
            Event::Comment(e) => {
                content.extend_from_slice(b"<!--");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"-->");
            }
            Event::Decl(e) => {
                content.extend_from_slice(b"<?");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"?>");
            }
            Event::PI(e) => {
                content.extend_from_slice(b"<?");
                content.extend_from_slice(e.as_ref());
                content.extend_from_slice(b"?>");
            }
            Event::DocType(e) => {
                content.extend_from_slice(b"<!DOCTYPE ");
                content.extend_from_slice(e.as_ref());
                content.push(b'>');
            }
            Event::Eof => anyhow::bail!("Unexpected EOF while parsing SVG"),
            _ => {}
        }
    }

    Ok(content)
}

/// Write img element as placeholder for extracted SVG
fn write_img_placeholder(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    svg: &Svg,
    ctx: &HtmlContext<'_>,
) -> Result<()> {
    let filename = svg.filename(ctx.config);
    let output_dir = ctx.html_path.parent().context("Invalid html path")?;
    let full_path = output_dir.join(&filename);

    // Build src attribute
    let src = url_from_output_path(&full_path, ctx.config).unwrap_or_else(|_| filename.clone());

    // Build style attribute with scaled dimensions
    let scale = ctx.config.get_scale();
    let (w, h) = (svg.size.0 / scale, svg.size.1 / scale);
    let mut style = String::with_capacity(40);
    let _ = write!(style, "width:{w}px;height:{h}px;");

    // Write img element
    let mut img = BytesStart::new("img");
    img.push_attribute(("src", src.as_str()));
    img.push_attribute(("style", style.as_str()));
    writer.write_event(Event::Start(img))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::events::attributes::Attribute;
    use quick_xml::name::QName;

    #[test]
    fn test_capture_svg_content() {
        // Input XML fragment (simulating content after <svg> tag)
        // Note: capture_svg_content expects to read until </svg>
        // We must provide a full SVG so the reader tracks the opening tag
        let xml = r#"<svg><rect x="0" y="0" width="10" height="10" /></svg>"#;
        let mut reader = Reader::from_str(xml);
        // reader.config_mut().trim_text(true);
        reader.config_mut().check_end_names = false;

        // Advance past the opening <svg> tag to simulate the state when capture_svg_content is called
        let _ = reader.read_event().unwrap();

        let attrs = vec![];
        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        // Should wrap in <svg>...</svg>
        assert_eq!(s, r#"<svg><rect x="0" y="0" width="10" height="10"/></svg>"#);
    }

    #[test]
    fn test_capture_svg_content_nested() {
        let xml = r#"<svg><g><svg viewBox="0 0 10 10"><rect /></svg></g></svg>"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = false;

        // Advance past the opening <svg> tag
        let _ = reader.read_event().unwrap();

        let attrs = vec![];
        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        assert_eq!(s, r#"<svg><g><svg viewBox="0 0 10 10"><rect/></svg></g></svg>"#);
    }

    #[test]
    fn test_capture_svg_content_with_attrs() {
        let xml = r#"<svg><rect /></svg>"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().check_end_names = false;
        let _ = reader.read_event().unwrap();

        let attrs = vec![
            Attribute { key: QName(b"width"), value: b"100".as_slice().into() },
            Attribute { key: QName(b"height"), value: b"100".as_slice().into() },
        ];

        let captured = capture_svg_content(&mut reader, &attrs).unwrap();
        let s = String::from_utf8(captured).unwrap();

        assert_eq!(s, r#"<svg width="100" height="100"><rect/></svg>"#);
    }
}
