use crate::config::SiteConfig;
use crate::utils::slug::slugify_fragment;
use crate::utils::svg::{compress_svgs_parallel, extract_svg_element, HtmlContext, Svg};
use anyhow::Result;
use quick_xml::{
    events::{BytesEnd, BytesStart, Event},
    Reader, Writer,
};
use std::io::Cursor;
use std::path::Path;
use std::str;

use super::common::{
    create_xml_reader, rebuild_elem, rebuild_elem_try, XmlWriter,
};
use super::head::write_head_content;
use super::link::process_link_value;

pub fn process_html(
    html_path: &Path,
    content: &[u8],
    config: &'static SiteConfig,
) -> Result<Vec<u8>> {
    let mut ctx = HtmlContext::new(config, html_path);
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(content.len())));
    let mut reader = create_xml_reader(content);
    let mut svgs = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(elem)) => {
                handle_start_element(&elem, &mut reader, &mut writer, &mut ctx, &mut svgs)?;
            }
            Ok(Event::End(elem)) => {
                handle_end_element(&elem, &mut writer, config)?;
            }
            Ok(Event::Eof) => break,
            Ok(event) => writer.write_event(event)?,
            Err(e) => anyhow::bail!(
                "XML parse error at position {}: {:?}",
                reader.error_position(),
                e
            ),
        }
    }

    // Compress SVGs in parallel
    if ctx.extract_svg && !svgs.is_empty() {
        compress_svgs_parallel(&svgs, html_path, config)?;
    }

    Ok(writer.into_inner().into_inner())
}

fn handle_start_element(
    elem: &BytesStart<'_>,
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    ctx: &mut HtmlContext<'_>,
    svgs: &mut Vec<Svg>,
) -> Result<()> {
    match elem.name().as_ref() {
        b"html" => write_html_with_lang(elem, writer, ctx.config)?,
        b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
            write_heading_with_slugified_id(elem, writer, ctx.config)?;
        }
        b"svg" if ctx.extract_svg => {
            if let Some(svg) = extract_svg_element(reader, writer, elem, ctx)? {
                svgs.push(svg);
            }
        }
        b"img" if ctx.config.build.css.auto_enhance => {
            write_img_with_color_invert(elem, writer, ctx.config)?;
        }
        _ => write_element_with_processed_links(elem, writer, ctx.config)?,
    }
    Ok(())
}

fn handle_end_element(
    elem: &BytesEnd<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    match elem.name().as_ref() {
        b"head" => write_head_content(writer, config)?,
        _ => writer.write_event(Event::End(elem.to_owned()))?,
    }
    Ok(())
}

/// Write `<html>` element with `lang` attribute.
pub fn write_html_with_lang(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &SiteConfig,
) -> Result<()> {
    let mut elem = elem.to_owned();
    elem.push_attribute(("lang", config.base.language.as_str()));
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

/// Write heading element with slugified `id` attribute.
pub fn write_heading_with_slugified_id(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &'static SiteConfig,
) -> Result<()> {
    let new_elem = rebuild_elem(elem, |key, value| {
        if key == b"id" {
            let v = str::from_utf8(value.as_ref()).unwrap_or_default();
            slugify_fragment(v, config).into_bytes().into()
        } else {
            value.into_owned().into()
        }
    });
    writer.write_event(Event::Start(new_elem))?;
    Ok(())
}

/// Write element with processed `href` and `src` attributes.
pub fn write_element_with_processed_links(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &'static SiteConfig,
) -> Result<()> {
    let new_elem = rebuild_elem_try(elem, |key, value| {
        if matches!(key, b"href" | b"src") {
            process_link_value(&value, config)
        } else {
            Ok(value.into_owned().into())
        }
    })?;
    writer.write_event(Event::Start(new_elem))?;
    Ok(())
}

/// Write `<img>` element with `color-invert` class for SVG dark mode adaptation.
///
/// Only adds `color-invert` to SVG images (`.svg`, `.svgz`) for proper dark mode support.
/// Non-SVG images (photos, etc.) are left unchanged to preserve their original colors.
/// Also processes `src` attribute for path normalization.
pub fn write_img_with_color_invert(
    elem: &BytesStart<'_>,
    writer: &mut XmlWriter,
    config: &'static SiteConfig,
) -> Result<()> {
    // Check if this is an SVG image
    let is_svg = elem.attributes().filter_map(|a| a.ok()).any(|attr| {
        attr.key.as_ref() == b"src" && {
            let src = str::from_utf8(attr.value.as_ref()).unwrap_or_default();
            src.ends_with(".svg") || src.ends_with(".svgz")
        }
    });

    // For non-SVG images, just process links normally
    if !is_svg {
        return write_element_with_processed_links(elem, writer, config);
    }

    // For SVG images, add color-invert class
    let mut has_class = false;

    let new_elem = rebuild_elem_try(elem, |key, value| {
        match key {
            b"src" => process_link_value(&value, config),
            b"class" => {
                has_class = true;
                // Append color-invert to existing classes
                let existing = str::from_utf8(value.as_ref()).unwrap_or_default();
                if existing.split_whitespace().any(|c| c == "color-invert") {
                    // Already has color-invert, keep as-is
                    Ok(value.into_owned().into())
                } else {
                    Ok(format!("{} color-invert", existing).into_bytes().into())
                }
            }
            _ => Ok(value.into_owned().into()),
        }
    })?;

    // Add class attribute if not present
    let new_elem = if has_class {
        new_elem
    } else {
        let mut elem = new_elem;
        elem.push_attribute(("class", "color-invert"));
        elem
    };

    writer.write_event(Event::Start(new_elem))?;
    Ok(())
}
