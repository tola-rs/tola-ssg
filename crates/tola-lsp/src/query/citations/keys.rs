//! Where the keys of a YAML bibliography sit in its text, and where one entry's key is written.

use std::ops::Range;

use anyhow::Result;
use lsp_types::Location;
use tola_typst::TypstWorld;
use tola_typst::typst::World;
use tola_typst::typst::syntax::Source;

use super::sources::Declared;

/// Where one entry's key is written in the bibliography file that declares it.
pub(super) fn key_location(
    world: &TypstWorld,
    declared: &Declared<'_>,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<Location>> {
    let Ok(bytes) = world.file(declared.file) else {
        return Ok(None);
    };
    let Ok(text) = std::str::from_utf8(bytes.as_slice()) else {
        return Ok(None);
    };
    let source = Source::new(declared.file, text.to_owned());
    let Some(range) = crate::position::utf16_range(source.lines(), declared.range.clone()) else {
        return Ok(None);
    };
    let Some(uri) = crate::identity::client_uri(declared.file, client_root) else {
        return Ok(None);
    };
    Ok(Some(Location { uri, range }))
}

/// Where the keys of a YAML bibliography sit in its text.
///
/// A bibliography's keys are the scalars of its outermost mapping, so the scanner reports one from
/// each depth-one scalar. Nothing else is read: the entries themselves come from Hayagriva.
pub(super) fn yaml_keys(text: &str) -> Vec<(String, Range<usize>)> {
    let mut keys = Keys::default();
    let mut parser = yaml_rust::parser::Parser::new(text.chars());
    // A file the scanner cannot read declares no entries either, so partial positions are harmless.
    let _ = parser.load(&mut keys, true);
    byte_ranges(text, keys.written)
        .into_iter()
        .filter_map(|(key, range)| Some((key, yaml_key_range(text, range)?)))
        .collect()
}

fn yaml_key_range(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(range.start)?;
    if quote != b'\'' && quote != b'"' {
        return Some(range);
    }
    // A decoded scalar's length cannot locate escaped spelling or its closing quote.
    let start = range.start + 1;
    let mut cursor = start;
    while let Some(&byte) = bytes.get(cursor) {
        if byte == quote {
            if quote == b'\'' && bytes.get(cursor + 1) == Some(&quote) {
                cursor += 2;
                continue;
            }
            return Some(start..cursor);
        }
        if quote == b'"' && byte == b'\\' {
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    None
}

/// The byte ranges of the keys one YAML scanner reported.
///
/// The scanner counts characters, while every range this module stores addresses bytes; one sweep
/// over the text maps the starts and ends both.
fn byte_ranges(text: &str, written: Vec<(String, Range<usize>)>) -> Vec<(String, Range<usize>)> {
    let mut offsets: Vec<usize> = written
        .iter()
        .flat_map(|(_, range)| [range.start, range.end])
        .collect();
    offsets.sort_unstable();
    offsets.dedup();
    let mut bytes = vec![0; offsets.len()];
    let mut cursor = 0;
    let mut byte = 0;
    for (character, ch) in text.chars().enumerate() {
        while cursor < offsets.len() && offsets[cursor] == character {
            bytes[cursor] = byte;
            cursor += 1;
        }
        byte += ch.len_utf8();
    }
    while cursor < offsets.len() {
        bytes[cursor] = byte;
        cursor += 1;
    }
    let byte_of = |offset: usize| {
        let index = offsets.binary_search(&offset).ok()?;
        Some(bytes[index])
    };
    written
        .into_iter()
        .filter_map(|(key, range)| Some((key, byte_of(range.start)?..byte_of(range.end)?)))
        .collect()
}

#[derive(Default)]
struct Keys {
    /// How deep the scanner is, where one is the depth a key sits at.
    depth: usize,
    written: Vec<(String, Range<usize>)>,
}

impl yaml_rust::parser::MarkedEventReceiver for Keys {
    fn on_event(&mut self, event: yaml_rust::parser::Event, mark: yaml_rust::scanner::Marker) {
        match event {
            yaml_rust::parser::Event::MappingStart(..) => self.depth += 1,
            yaml_rust::parser::Event::MappingEnd => self.depth = self.depth.saturating_sub(1),
            yaml_rust::parser::Event::Scalar(name, ..) if self.depth == 1 => {
                let start = mark.index();
                self.written
                    .push((name.clone(), start..start + name.chars().count()));
            }
            _ => {}
        }
    }
}
