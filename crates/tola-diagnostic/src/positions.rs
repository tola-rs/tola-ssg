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
    src: Arc<str>,
    span: Range<usize>,
    val_span: Option<Range<usize>>,
    file: Option<Arc<Path>>,
}

/// One written key or array element and the source that contains it.
#[derive(Debug, Clone, Copy)]
pub struct WrittenPosition<'a> {
    pos: &'a Position,
    span: &'a Range<usize>,
}

impl WrittenPosition<'_> {
    pub fn file(&self) -> Option<&Path> {
        self.pos.file.as_deref()
    }

    pub fn value(self) -> Self {
        Self {
            span: self.pos.val_span.as_ref().unwrap_or(self.span),
            ..self
        }
    }

    pub fn excerpt(&self) -> Option<SourceExcerpt> {
        SourceExcerpt::of(&self.pos.src, self.span.clone())
    }

    pub fn range(&self) -> SourceRange {
        source_range(&self.pos.src, self.span.clone())
    }
}

impl SourcePositions {
    pub fn of(doc: &Document<Arc<str>>, src: Arc<str>) -> Self {
        let mut out = Self::default();
        collect_keys(doc.as_table(), &mut String::new(), &src, &mut out.keys);
        out
    }

    pub fn in_root(root: PathBuf) -> Self {
        Self {
            root: Some(root),
            keys: BTreeMap::new(),
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn copy_position(&mut self, src: &Self, from: &str, to: &str, file: &Path) {
        if let Some(pos) = src.keys.get(from) {
            self.keys.insert(
                to.to_owned(),
                Position {
                    file: Some(Arc::from(file)),
                    ..pos.clone()
                },
            );
        }
    }

    pub fn copy_subtree(&mut self, src: &Self, from: &str, to: &str, file: &Path) {
        let file = Arc::<Path>::from(file);
        let prefix = format!("{from}/");
        let entries = src
            .keys
            .get_key_value(from)
            .into_iter()
            .chain(descendant_keys(&src.keys, &prefix));
        for (k, pos) in entries {
            let suffix = &k[from.len()..];
            self.keys.insert(
                format!("{to}{suffix}"),
                Position {
                    file: Some(Arc::clone(&file)),
                    ..pos.clone()
                },
            );
        }
    }

    pub fn remove_subtree(&mut self, ptr: &str) {
        self.keys.remove(ptr);
        let prefix = format!("{ptr}/");
        let keys: Vec<_> = descendant_keys(&self.keys, &prefix)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.keys.remove(&k);
        }
    }

    pub fn at_pointer(&self, ptr: &str) -> Option<WrittenPosition<'_>> {
        self.keys.get(ptr).map(|pos| WrittenPosition {
            pos,
            span: &pos.span,
        })
    }

    pub fn at(&self, dotted: &str) -> Option<WrittenPosition<'_>> {
        let mut ptr = String::new();
        for seg in dotted.split('.') {
            push_pointer(&mut ptr, seg);
        }
        self.at_pointer(&ptr)
    }

    pub fn excerpt(&self, dotted: &str) -> Option<SourceExcerpt> {
        self.at(dotted)?.excerpt()
    }

    pub fn written(&self, dotted: &str) -> Option<SourceRange> {
        Some(self.at(dotted)?.range())
    }
}

fn descendant_keys<'a>(
    keys: &'a BTreeMap<String, Position>,
    prefix: &'a str,
) -> impl Iterator<Item = (&'a String, &'a Position)> {
    keys.range::<str, _>((Bound::Included(prefix), Bound::Unbounded))
        .take_while(move |(k, _)| k.starts_with(prefix))
}

fn collect_keys(
    tbl: &dyn TableLike,
    path: &mut String,
    src: &Arc<str>,
    keys: &mut BTreeMap<String, Position>,
) {
    let base = path.len();
    for (name, item) in tbl.iter() {
        if item.is_none() {
            continue;
        }
        push_pointer(path, name);
        // A table can write its key in a header instead of a value line.
        record(
            keys,
            path,
            src,
            tbl.key(name).and_then(|k| k.span()),
            item.span(),
        );
        match item {
            Item::Table(t) => collect_keys(t, path, src, keys),
            Item::Value(v) => collect_value(v, path, src, keys),
            Item::ArrayOfTables(tables) => {
                collect_seq(
                    tables.iter().map(|t| (t, t.span())),
                    path,
                    src,
                    keys,
                    |t, p, s, k| collect_keys(t, p, s, k),
                );
            }
            Item::None => {}
        }
        path.truncate(base);
    }
}

fn collect_value(
    val: &Value,
    path: &mut String,
    src: &Arc<str>,
    keys: &mut BTreeMap<String, Position>,
) {
    if let Some(t) = val.as_inline_table() {
        collect_keys(t, path, src, keys);
    } else if let Some(arr) = val.as_array() {
        collect_seq(
            arr.iter().map(|v| (v, v.span())),
            path,
            src,
            keys,
            collect_value,
        );
    }
}

fn collect_seq<'a, T: 'a>(
    items: impl Iterator<Item = (&'a T, Option<Range<usize>>)>,
    path: &mut String,
    src: &Arc<str>,
    keys: &mut BTreeMap<String, Position>,
    mut f: impl FnMut(&T, &mut String, &Arc<str>, &mut BTreeMap<String, Position>),
) {
    let base = path.len();
    for (idx, (item, span)) in items.enumerate() {
        push_pointer(path, &idx.to_string());
        record(keys, path, src, None, span);
        f(item, path, src, keys);
        path.truncate(base);
    }
}

fn record(
    keys: &mut BTreeMap<String, Position>,
    path: &str,
    src: &Arc<str>,
    key_span: Option<Range<usize>>,
    val_span: Option<Range<usize>>,
) {
    if let Some(span) = key_span.or_else(|| val_span.clone()) {
        keys.insert(
            path.to_owned(),
            Position {
                src: Arc::clone(src),
                span,
                val_span,
                file: None,
            },
        );
    }
}

/// Append a literal key or sequence index to a JSON Pointer.
pub fn push_pointer(ptr: &mut String, seg: &str) {
    ptr.push('/');
    for ch in seg.chars() {
        match ch {
            '~' => ptr.push_str("~0"),
            '/' => ptr.push_str("~1"),
            ch => ptr.push(ch),
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
pub fn key_at_span(doc: &Document<Arc<str>>, span: &Range<usize>) -> Option<String> {
    table_key_at_span(doc.as_table(), "", span)
}

fn table_key_at_span(tbl: &dyn TableLike, prefix: &str, span: &Range<usize>) -> Option<String> {
    tbl.iter()
        .find_map(|(name, item)| item_key_at_span(item, joined_path(prefix, name), span))
}

fn item_key_at_span(item: &Item, path: String, span: &Range<usize>) -> Option<String> {
    if let Some(tables) = item.as_array_of_tables() {
        return tables.iter().enumerate().find_map(|(idx, t)| {
            table_key_at_span(t, &joined_path(&path, &idx.to_string()), span)
        });
    }
    if let Some(t) = item.as_table_like()
        && let Some(k) = table_key_at_span(t, &path, span)
    {
        return Some(k);
    }
    item.span()
        .filter(|v| v.start <= span.start && span.end <= v.end)
        .map(|_| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(text: &str) -> SourcePositions {
        let text = Arc::<str>::from(text);
        let doc = Document::parse(Arc::clone(&text)).unwrap();
        SourcePositions::of(&doc, text)
    }

    #[test]
    fn copied_array_spans() {
        let entry = positions("items = [{ name = 'entry' }]\n");
        let frag = positions("\nitems = [{ name = 'included' }]\n");
        let entry_path = Path::new("site/tola.toml");
        let frag_path = Path::new("site/config/common.toml");
        let mut combined = SourcePositions::in_root(PathBuf::from("site"));
        combined.copy_subtree(&entry, "", "", entry_path);
        combined.copy_subtree(&frag, "/items/0", "/items/1", frag_path);

        let written = combined.at("items.1.name").unwrap();
        assert_eq!(written.file(), Some(frag_path));
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
        assert_eq!(combined.at("items.1.name").unwrap().file(), Some(frag_path));
    }

    #[test]
    fn literal_key_segments() {
        let pos = positions("'site.title' = 'literal'\n[site]\ntitle = 'nested'\n'a~/b' = true\n");
        assert_eq!(pos.written("site.title").unwrap().start.line, 2);
        assert_eq!(pos.at_pointer("/site.title").unwrap().range().start.line, 0);
        assert_eq!(
            pos.at_pointer("/site/a~0~1b").unwrap().range().start.line,
            3
        );
    }
}
