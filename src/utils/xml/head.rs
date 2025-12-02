use crate::config::SiteConfig;
use anyhow::Result;
use quick_xml::events::{BytesEnd, Event};
use std::io::Write;

use super::assets::{compute_asset_href, compute_stylesheet_href, get_icon_mime_type};
use super::common::{write_empty_elem, write_script, write_text_element, XmlWriter};

/// Write `<head>` section content before closing tag.
pub fn write_head_content(writer: &mut XmlWriter, config: &'static SiteConfig) -> Result<()> {
    let head = &config.build.head;

    if !config.base.title.is_empty() {
        write_text_element(writer, "title", &config.base.title)?;
    }
    if !config.base.description.is_empty() {
        write_empty_elem(
            writer,
            "meta",
            &[
                ("name", "description"),
                ("content", &config.base.description),
            ],
        )?;
    }

    if let Some(icon) = &head.icon {
        let href = compute_asset_href(icon, config)?;
        write_empty_elem(
            writer,
            "link",
            &[
                ("rel", "shortcut icon"),
                ("href", &href),
                ("type", get_icon_mime_type(icon)),
            ],
        )?;
    }

    for style in &head.styles {
        let href = compute_asset_href(style, config)?;
        write_empty_elem(writer, "link", &[("rel", "stylesheet"), ("href", &href)])?;
    }

    if config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
    {
        let href = compute_stylesheet_href(input, config)?;
        write_empty_elem(writer, "link", &[("rel", "stylesheet"), ("href", &href)])?;
    }

    // Scripts
    for script in &head.scripts {
        let src = compute_asset_href(script.path(), config)?;
        write_script(writer, &src, script.is_defer(), script.is_async())?;
    }

    // Raw HTML elements (trusted input)
    for raw in &head.elements {
        writer.get_mut().write_all(raw.as_bytes())?;
    }

    writer.write_event(Event::End(BytesEnd::new("head")))?;
    Ok(())
}
