//! Where one parsed TOML document writes each dotted key, and the excerpts that point there.

use std::sync::Arc;

use crate::{SourcePosition, SourceRange};

/// Where every key of one parsed configuration document is written, by dotted path.
#[derive(Debug, Clone)]
pub struct SourcePositions {
    source: Arc<str>,
    keys: std::collections::BTreeMap<String, std::ops::Range<usize>>,
}

impl Default for SourcePositions {
    fn default() -> Self {
        Self {
            source: Arc::from(""),
            keys: std::collections::BTreeMap::new(),
        }
    }
}

impl SourcePositions {
    /// Record every key of one parsed document against the exact source that produced it.
    ///
    /// A path written more than once keeps the span of its first occurrence.
    pub fn of(document: &toml_edit::Document<Arc<str>>, source: Arc<str>) -> Self {
        let mut keys = std::collections::BTreeMap::new();
        collect_table_keys(document.as_table(), "", &mut keys);
        Self { source, keys }
    }

    /// The written key a dotted configuration path names, with its line and highlight.
    pub fn excerpt(&self, dotted: &str) -> Option<SourceExcerpt> {
        SourceExcerpt::of(&self.source, self.keys.get(dotted)?.clone())
    }

    /// The exact span the document writes one dotted configuration path in.
    pub fn written(&self, dotted: &str) -> Option<SourceRange> {
        source_range(&self.source, self.keys.get(dotted)?.clone())
    }
}

fn collect_table_keys(
    table: &toml_edit::Table,
    prefix: &str,
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
) {
    for (name, _) in table.iter() {
        let Some((key, item)) = table.get_key_value(name) else {
            continue;
        };
        let path = joined_path(prefix, name);
        // A table header writes its key without a value line, so the table's own span is where
        // that key is written.
        let span = key.span().or_else(|| match item {
            toml_edit::Item::Table(table) => table.span(),
            _ => None,
        });
        if let Some(span) = span {
            record_key_span(keys, &path, span);
        }
        collect_item_keys(item, &path, keys);
    }
}

fn collect_item_keys(
    item: &toml_edit::Item,
    path: &str,
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
) {
    match item {
        toml_edit::Item::Table(table) => collect_table_keys(table, path, keys),
        toml_edit::Item::ArrayOfTables(elements) => {
            for (index, table) in elements.iter().enumerate() {
                let element = joined_path(path, &index.to_string());
                // An element has no key of its own, so its header line is where it starts.
                if let Some(span) = table.span() {
                    record_key_span(keys, &element, span);
                }
                collect_table_keys(table, &element, keys);
            }
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(inline)) => {
            for (name, _) in inline.iter() {
                let Some((key, item)) = inline.get_key_value(name) else {
                    continue;
                };
                let path = joined_path(path, name);
                if let Some(span) = key.span() {
                    record_key_span(keys, &path, span);
                }
                collect_item_keys(item, &path, keys);
            }
        }
        toml_edit::Item::Value(_) | toml_edit::Item::None => {}
    }
}

/// Keep the first key a path names, so a later duplicate cannot move an author elsewhere.
fn record_key_span(
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
    path: &str,
    span: std::ops::Range<usize>,
) {
    keys.entry(path.to_owned()).or_insert(span);
}

fn joined_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// The dotted key a schema span sits inside, when the document writes one there.
pub fn key_at_span(
    document: &toml_edit::Document<Arc<str>>,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    table_key_at_span(document.as_table(), "", span)
}

fn table_key_at_span(
    table: &toml_edit::Table,
    prefix: &str,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    table
        .iter()
        .find_map(|(name, item)| item_key_at_span(item, &joined_path(prefix, name), span))
}

fn item_key_at_span(
    item: &toml_edit::Item,
    path: &str,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    match item {
        toml_edit::Item::Table(table) => table_key_at_span(table, path, span).or_else(|| {
            table
                .span()
                .filter(|table| table.start <= span.start && span.end <= table.end)
                .map(|_| path.to_owned())
        }),
        toml_edit::Item::ArrayOfTables(tables) => {
            tables.iter().enumerate().find_map(|(index, table)| {
                table_key_at_span(table, &joined_path(path, &index.to_string()), span)
            })
        }
        toml_edit::Item::Value(value @ toml_edit::Value::InlineTable(table)) => table
            .iter()
            .find_map(|(name, _)| {
                let (_, value) = table.get_key_value(name)?;
                item_key_at_span(value, &joined_path(path, name), span)
            })
            .or_else(|| {
                value
                    .span()
                    .filter(|value| value.start <= span.start && span.end <= value.end)
                    .map(|_| path.to_owned())
            }),
        toml_edit::Item::Value(value) => value
            .span()
            .filter(|value| value.start <= span.start && span.end <= value.end)
            .map(|_| path.to_owned()),
        toml_edit::Item::None => None,
    }
}

/// The written line one span covers, with the highlight that points inside it.
pub struct SourceExcerpt {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub highlight: (usize, usize),
}

impl SourceExcerpt {
    /// The written line one source span covers, with the highlight that points inside it.
    pub fn of(source: &str, span: std::ops::Range<usize>) -> Option<Self> {
        let mut start = span.start.min(source.len());
        start = (0..=start)
            .rev()
            .find(|offset| source.is_char_boundary(*offset))?;
        if start == source.len() && source.ends_with('\n') {
            start -= 1;
        }
        let line_start = source[..start].rfind('\n').map_or(0, |offset| offset + 1);
        let line_end = source[start..]
            .find('\n')
            .map_or(source.len(), |offset| start + offset);
        let mut highlight = (start - line_start, span.end.min(line_end) - line_start);
        if highlight.1 <= highlight.0 {
            highlight.0 = highlight.0.saturating_sub(1);
            highlight.1 = highlight.0 + usize::from(line_end > line_start);
        }
        Some(Self {
            line: source[..line_start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            column: source[line_start..start].chars().count() + 1,
            text: source[line_start..line_end].to_owned(),
            highlight,
        })
    }
}

/// The exact UTF-16 range one byte span covers, clamped to the text the document holds.
fn source_range(source: &str, span: std::ops::Range<usize>) -> Option<SourceRange> {
    Some(SourceRange {
        start: source_position(source, span.start)?,
        end: source_position(source, span.end)?,
    })
}

/// The zero-based UTF-16 position one byte offset names.
fn source_position(source: &str, offset: usize) -> Option<SourcePosition> {
    let offset = offset.min(source.len());
    let offset = (0..=offset)
        .rev()
        .find(|candidate| source.is_char_boundary(*candidate))?;
    let line_start = source[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    Some(SourcePosition {
        line: source[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count(),
        character: source[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum(),
    })
}
