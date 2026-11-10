//! Written keys and schema error spans in a parsed TOML document.

use std::{collections::BTreeMap, ops::Range, sync::Arc};

use toml_edit::{Document, Item, TableLike};

use crate::source::{SourceExcerpt, SourceRange, source_range};

/// Where every key of one parsed configuration document is written, by dotted path.
#[derive(Debug, Clone, Default)]
pub struct SourcePositions {
    source: Arc<str>,
    keys: BTreeMap<String, Range<usize>>,
}

impl SourcePositions {
    /// Record every key of one parsed document against the exact source that produced it.
    ///
    /// A path written more than once keeps the span of its first occurrence.
    pub fn of(document: &Document<Arc<str>>, source: Arc<str>) -> Self {
        let mut keys = BTreeMap::new();
        collect_keys(document.as_table(), "", &mut keys);
        Self { source, keys }
    }

    /// The written key a dotted configuration path names, with its line and highlight.
    pub fn excerpt(&self, dotted: &str) -> Option<SourceExcerpt> {
        SourceExcerpt::of(&self.source, self.keys.get(dotted)?.clone())
    }

    /// The exact span the document writes one dotted configuration path in.
    pub fn written(&self, dotted: &str) -> Option<SourceRange> {
        Some(source_range(&self.source, self.keys.get(dotted)?.clone()))
    }
}

fn collect_keys(table: &dyn TableLike, prefix: &str, keys: &mut BTreeMap<String, Range<usize>>) {
    for (name, item) in table.iter() {
        if item.is_none() {
            continue;
        }
        let path = joined_path(prefix, name);
        let span = table.key(name).and_then(|key| key.span()).or_else(|| {
            // A table can write its key in a header instead of a value line.
            item.as_table().and_then(|table| table.span())
        });
        if let Some(span) = span {
            keys.entry(path.clone()).or_insert(span);
        }
        if let Some(table) = item.as_table_like() {
            collect_keys(table, &path, keys);
        } else if let Some(tables) = item.as_array_of_tables() {
            for (index, table) in tables.iter().enumerate() {
                let element = joined_path(&path, &index.to_string());
                // The element has no key of its own. Point at its header.
                if let Some(span) = table.span() {
                    keys.entry(element.clone()).or_insert(span);
                }
                collect_keys(table, &element, keys);
            }
        }
    }
}

fn joined_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// The dotted key a schema span sits inside, when the document writes one there.
pub fn key_at_span(document: &Document<Arc<str>>, span: &Range<usize>) -> Option<String> {
    table_key_at_span(document.as_table(), "", span)
}

fn table_key_at_span(table: &dyn TableLike, prefix: &str, span: &Range<usize>) -> Option<String> {
    table
        .iter()
        .find_map(|(name, item)| item_key_at_span(item, joined_path(prefix, name), span))
}

fn item_key_at_span(item: &Item, path: String, span: &Range<usize>) -> Option<String> {
    if let Some(tables) = item.as_array_of_tables() {
        return tables.iter().enumerate().find_map(|(index, table)| {
            table_key_at_span(table, &joined_path(&path, &index.to_string()), span)
        });
    }
    if let Some(table) = item.as_table_like()
        && let Some(key) = table_key_at_span(table, &path, span)
    {
        return Some(key);
    }
    item.span()
        .filter(|value| value.start <= span.start && span.end <= value.end)
        .map(|_| path)
}
