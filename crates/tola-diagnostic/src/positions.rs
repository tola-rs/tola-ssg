//! Written keys and schema error spans in TOML documents.

use std::{
    collections::BTreeMap,
    ops::{Bound, Range},
    path::{Path, PathBuf},
    sync::Arc,
};

use toml_edit::{Document, Item, TableLike, Value};

use crate::source::{SourceExcerpt, SourceRange, source_range};

/// Where configuration keys and array elements were written, by JSON Pointer.
#[derive(Debug, Clone, Default)]
pub struct SourcePositions {
    root: Option<PathBuf>,
    keys: BTreeMap<String, Position>,
}

#[derive(Debug, Clone)]
struct Position {
    source: Arc<str>,
    span: Range<usize>,
    value_span: Option<Range<usize>>,
    file: Option<Arc<Path>>,
}

/// One written key or array element and the source that contains it.
#[derive(Debug, Clone, Copy)]
pub struct WrittenPosition<'a> {
    position: &'a Position,
    span: &'a Range<usize>,
}

impl WrittenPosition<'_> {
    /// The original source file when the position is attached to one.
    pub fn file(&self) -> Option<&Path> {
        self.position.file.as_deref()
    }

    /// Point at the value if it has its own span, or keep the key's position.
    pub fn value(self) -> Self {
        Self {
            span: self.position.value_span.as_ref().unwrap_or(self.span),
            ..self
        }
    }

    /// The written line and the highlight inside it.
    pub fn excerpt(&self) -> Option<SourceExcerpt> {
        SourceExcerpt::of(&self.position.source, self.span.clone())
    }

    /// The exact range in the written source.
    pub fn range(&self) -> SourceRange {
        source_range(&self.position.source, self.span.clone())
    }
}

impl SourcePositions {
    /// Record one parsed document against the exact source that produced it.
    pub fn of(document: &Document<Arc<str>>, source: Arc<str>) -> Self {
        let mut positions = Self::default();
        collect_keys(
            document.as_table(),
            &mut String::new(),
            &source,
            &mut positions.keys,
        );
        positions
    }

    /// Start an index for sources combined under one site root.
    pub fn in_root(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            keys: BTreeMap::new(),
        }
    }

    /// The root under which combined source files are displayed.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Copy one written position without copying its descendants.
    pub fn copy_position(&mut self, source: &Self, from: &str, to: &str, file: &Path) {
        if let Some(position) = source.keys.get(from) {
            self.keys.insert(
                to.to_owned(),
                Position {
                    file: Some(Arc::from(file)),
                    ..position.clone()
                },
            );
        }
    }

    /// Copy a written value and its descendants, retaining their original text and spans.
    pub fn copy_subtree(&mut self, source: &Self, from: &str, to: &str, file: &Path) {
        let file = Arc::<Path>::from(file);
        let descendants = format!("{from}/");
        let entries = source.keys.get_key_value(from).into_iter().chain(
            source
                .keys
                .range::<str, _>((Bound::Included(descendants.as_str()), Bound::Unbounded))
                .take_while(|(key, _)| key.starts_with(&descendants)),
        );
        for (key, position) in entries {
            let suffix = &key[from.len()..];
            self.keys.insert(
                format!("{to}{suffix}"),
                Position {
                    file: Some(Arc::clone(&file)),
                    ..position.clone()
                },
            );
        }
    }

    /// Drop a written value and its descendants when it is replaced.
    pub fn remove_subtree(&mut self, pointer: &str) {
        self.keys.remove(pointer);
        let prefix = format!("{pointer}/");
        let descendants: Vec<_> = self
            .keys
            .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in descendants {
            self.keys.remove(&key);
        }
    }

    /// The position at an exact JSON Pointer.
    pub fn at_pointer(&self, pointer: &str) -> Option<WrittenPosition<'_>> {
        self.keys.get(pointer).map(|position| WrittenPosition {
            position,
            span: &position.span,
        })
    }

    /// The position denoted by a schema's dotted path of static keys and indices.
    pub fn at(&self, dotted: &str) -> Option<WrittenPosition<'_>> {
        let mut pointer = String::new();
        for segment in dotted.split('.') {
            push_pointer(&mut pointer, segment);
        }
        self.at_pointer(&pointer)
    }

    /// The written key a dotted configuration path names, with its line and highlight.
    pub fn excerpt(&self, dotted: &str) -> Option<SourceExcerpt> {
        self.at(dotted)?.excerpt()
    }

    /// The exact span the document writes one dotted configuration path in.
    pub fn written(&self, dotted: &str) -> Option<SourceRange> {
        Some(self.at(dotted)?.range())
    }
}

fn collect_keys(
    table: &dyn TableLike,
    path: &mut String,
    source: &Arc<str>,
    keys: &mut BTreeMap<String, Position>,
) {
    let length = path.len();
    for (name, item) in table.iter() {
        if item.is_none() {
            continue;
        }
        push_pointer(path, name);
        // A table can write its key in a header instead of a value line.
        record(
            keys,
            path,
            source,
            table.key(name).and_then(|key| key.span()),
            item.span(),
        );
        match item {
            Item::Table(table) => collect_keys(table, path, source, keys),
            Item::Value(value) => collect_value(value, path, source, keys),
            Item::ArrayOfTables(tables) => {
                let array_length = path.len();
                for (index, table) in tables.iter().enumerate() {
                    push_pointer(path, &index.to_string());
                    record(keys, path, source, None, table.span());
                    collect_keys(table, path, source, keys);
                    path.truncate(array_length);
                }
            }
            Item::None => {}
        }
        path.truncate(length);
    }
}

fn collect_value(
    value: &Value,
    path: &mut String,
    source: &Arc<str>,
    keys: &mut BTreeMap<String, Position>,
) {
    if let Some(table) = value.as_inline_table() {
        collect_keys(table, path, source, keys);
    } else if let Some(array) = value.as_array() {
        let length = path.len();
        for (index, item) in array.iter().enumerate() {
            push_pointer(path, &index.to_string());
            record(keys, path, source, None, item.span());
            collect_value(item, path, source, keys);
            path.truncate(length);
        }
    }
}

fn record(
    keys: &mut BTreeMap<String, Position>,
    path: &str,
    source: &Arc<str>,
    key_span: Option<Range<usize>>,
    value_span: Option<Range<usize>>,
) {
    if let Some(span) = key_span.or_else(|| value_span.clone()) {
        keys.insert(
            path.to_owned(),
            Position {
                source: Arc::clone(source),
                span,
                value_span,
                file: None,
            },
        );
    }
}

/// Append a literal key or sequence index to a JSON Pointer.
pub fn push_pointer(pointer: &mut String, segment: &str) {
    pointer.push('/');
    for character in segment.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            character => pointer.push(character),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(text: &str) -> SourcePositions {
        let text = Arc::<str>::from(text);
        let document = Document::parse(Arc::clone(&text)).unwrap();
        SourcePositions::of(&document, text)
    }

    #[test]
    fn copied_array_elements_keep_their_original_file_and_ranges() {
        let entry = positions("items = [{ name = 'entry' }]\n");
        let fragment = positions("\nitems = [{ name = 'included' }]\n");
        let entry_path = Path::new("site/tola.toml");
        let fragment_path = Path::new("site/config/common.toml");
        let mut combined = SourcePositions::in_root(PathBuf::from("site"));
        combined.copy_subtree(&entry, "", "", entry_path);
        combined.copy_subtree(&fragment, "/items/0", "/items/1", fragment_path);

        let written = combined.at("items.1.name").unwrap();
        assert_eq!(written.file(), Some(fragment_path));
        let excerpt = written.value().excerpt().unwrap();
        assert_eq!(excerpt.line, 2);
        assert_eq!(
            &excerpt.text[excerpt.highlight.0..excerpt.highlight.1],
            "'included'"
        );
        assert_eq!(
            combined
                .at_pointer("/items/1")
                .unwrap()
                .range()
                .start
                .character,
            9
        );

        combined.remove_subtree("/items/0");
        assert!(combined.at("items.0.name").is_none());
        assert_eq!(
            combined.at("items.1.name").unwrap().file(),
            Some(fragment_path)
        );
    }

    #[test]
    fn literal_keys_keep_their_segments() {
        let positions =
            positions("'site.title' = 'literal'\n[site]\ntitle = 'nested'\n'a~/b' = true\n");
        assert_eq!(positions.written("site.title").unwrap().start.line, 2);
        assert_eq!(
            positions
                .at_pointer("/site.title")
                .unwrap()
                .range()
                .start
                .line,
            0
        );
        assert_eq!(
            positions
                .at_pointer("/site/a~0~1b")
                .unwrap()
                .range()
                .start
                .line,
            3
        );
    }
}
