use anyhow::Result;
use quick_xml::{
    events::{BytesEnd, BytesStart, BytesText, Event},
    Reader, Writer,
};
use std::borrow::Cow;
use std::io::Cursor;

pub type XmlWriter = Writer<Cursor<Vec<u8>>>;

#[inline]
pub fn create_xml_reader(content: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(false);
    reader.config_mut().enable_all_checks(false);
    reader
}

/// Rebuild an element with transformed attributes (avoids duplication bug).
pub fn rebuild_elem<F>(elem: &BytesStart<'_>, mut transform: F) -> BytesStart<'static>
where
    F: FnMut(&[u8], Cow<'_, [u8]>) -> Cow<'static, [u8]>,
{
    let tag = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let attrs: Vec<_> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key.as_ref().to_vec();
            let value = transform(attr.key.as_ref(), attr.value);
            (key, value)
        })
        .collect();

    let mut new_elem = BytesStart::new(tag);
    for (k, v) in attrs {
        new_elem.push_attribute((k.as_slice(), v.as_ref()));
    }
    new_elem
}

/// Rebuild an element with fallible attribute transformation.
pub fn rebuild_elem_try<F>(elem: &BytesStart<'_>, mut transform: F) -> Result<BytesStart<'static>>
where
    F: FnMut(&[u8], Cow<'_, [u8]>) -> Result<Cow<'static, [u8]>>,
{
    let tag = String::from_utf8_lossy(elem.name().as_ref()).into_owned();
    let attrs: Result<Vec<_>> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key.as_ref().to_vec();
            let value = transform(attr.key.as_ref(), attr.value)?;
            Ok((key, value))
        })
        .collect();

    let mut new_elem = BytesStart::new(tag);
    for (k, v) in attrs? {
        new_elem.push_attribute((k.as_slice(), v.as_ref()));
    }
    Ok(new_elem)
}

/// Write a text element: `<tag>text</tag>`.
#[inline]
pub fn write_text_element(writer: &mut XmlWriter, tag: &str, text: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

/// Write an empty element with attributes: `<tag attr1="val1" ... />`.
#[inline]
pub fn write_empty_elem(writer: &mut XmlWriter, tag: &str, attrs: &[(&str, &str)]) -> Result<()> {
    let mut elem = BytesStart::new(tag);
    for (k, v) in attrs {
        elem.push_attribute((*k, *v));
    }
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

/// Write a script element with optional defer/async.
pub fn write_script(
    writer: &mut XmlWriter,
    src: &str,
    defer: bool,
    async_attr: bool,
) -> Result<()> {
    let mut elem = BytesStart::new("script");
    elem.push_attribute(("src", src));
    if defer {
        elem.push_attribute(("defer", ""));
    }
    if async_attr {
        elem.push_attribute(("async", ""));
    }
    writer.write_event(Event::Start(elem))?;
    // Space ensures proper HTML parsing of script tags
    writer.write_event(Event::Text(BytesText::new(" ")))?;
    writer.write_event(Event::End(BytesEnd::new("script")))?;
    Ok(())
}
