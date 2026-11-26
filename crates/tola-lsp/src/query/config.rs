//! Configuration key completion and hover for `tola.toml`, read from its schema.

#[cfg(test)]
mod tests;

use std::ops::Range;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Hover, Position, TextEdit,
};
use tola_build::config::{field_documentation, field_paths, field_values};
use tola_diagnostic::toml::{field_replacement, immediate_key};
use tola_typst::typst::syntax::Lines;

use crate::position;
use crate::server::HostSections;

pub(crate) fn document(text: &str, position: Position, host: HostSections) -> Vec<CompletionItem> {
    let Some((lines, offset)) = at_offset(text, position) else {
        return Vec::new();
    };
    completion(&lines, offset, host)
}

fn at_offset(text: &str, pos: Position) -> Option<(Lines<String>, usize)> {
    let lines = Lines::new(text.to_owned());
    let offset = position::byte_offset(&lines, pos).ok()?;
    Some((lines, offset))
}

fn line_range_at(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |nl| nl + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |nl| offset + nl);
    start..end
}

fn parsed_with_blanked(text: &str, blanked: Range<usize>) -> Option<toml_edit::Document<String>> {
    let mut blanked_text = text.to_owned();
    blanked_text.replace_range(blanked.clone(), &" ".repeat(blanked.len()));
    blanked_text.parse::<toml_edit::Document<String>>().ok()
}

pub(crate) fn document_hover(text: &str, position: Position, host: HostSections) -> Option<Hover> {
    let (lines, offset) = at_offset(text, position)?;
    let field = at_field(&lines, offset)?;
    let range = position::utf16_range(&lines, field.range)?;
    let (path, docs) = field
        .paths
        .into_iter()
        .find_map(|path| documentation(host, &path).map(|docs| (path, docs)))?;
    Some(crate::protocol::markdown_hover(
        format!("`{path}`\n\n{docs}"),
        Some(range),
    ))
}

fn declared_paths(
    host: HostSections,
) -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    let core = field_paths().map(|f| (f.as_str(), field_documentation(f.as_str())));
    let host_fields = host
        .iter()
        .flat_map(|s| s.iter())
        .map(|(f, docs)| (f.as_str(), *docs));
    core.chain(host_fields)
}

fn documentation(host: HostSections, path: &str) -> Option<&'static str> {
    declared_paths(host)
        .find(|(field, _)| *field == path)
        .and_then(|(_, docs)| docs)
}

fn declares(host: HostSections, path: &str) -> bool {
    declared_paths(host).any(|(field, _)| field == path)
}

struct CursorField {
    range: Range<usize>,
    paths: Vec<String>,
}

impl CursorField {
    fn named(path: String, range: Range<usize>) -> Self {
        Self {
            range,
            paths: vec![path],
        }
    }

    fn within(mut self, member: String) -> Self {
        self.paths.push(member);
        self
    }
}

fn at_field<T: AsRef<str>>(lines: &Lines<T>, offset: usize) -> Option<CursorField> {
    key_path(lines, offset)
        .map(|(range, path)| CursorField::named(path, range))
        .or_else(|| value_path(lines, offset))
}

fn value_path<T: AsRef<str>>(lines: &Lines<T>, offset: usize) -> Option<CursorField> {
    let text = lines.text();
    if let Ok(doc) = toml_edit::Document::parse(text)
        && let Some(val) = enclosing_value(doc.as_table(), offset)
    {
        return Some(val);
    }
    value_on_its_line(lines, offset).map(|(range, path)| CursorField::named(path, range))
}

fn enclosing_value(root: &toml_edit::Table, offset: usize) -> Option<CursorField> {
    fn walk(
        table: &toml_edit::Table,
        path: &mut String,
        offset: usize,
        found: &mut Option<CursorField>,
    ) {
        for (key, item) in table.iter() {
            match item {
                toml_edit::Item::Value(val) => {
                    let Some(span) = val.span() else { continue };
                    if !span.contains(&offset) {
                        continue;
                    }
                    let member_path = child_path(path, key);
                    *found = Some(match member_in(val, &member_path, offset) {
                        Some(member) => member.within(member_path),
                        None => CursorField::named(member_path, span.clone()),
                    });
                }
                toml_edit::Item::Table(inner) => {
                    with_key(path, key, |path| walk(inner, path, offset, found));
                }
                toml_edit::Item::ArrayOfTables(inner) => with_key(path, key, |path| {
                    for table in inner {
                        walk(table, path, offset, found);
                    }
                }),
                toml_edit::Item::None => {}
            }
        }
    }

    let mut found = None;
    walk(root, &mut String::new(), offset, &mut found);
    found
}

fn member_in(val: &toml_edit::Value, path: &str, offset: usize) -> Option<CursorField> {
    match val {
        toml_edit::Value::InlineTable(table) => inline_member(table, path, offset),
        toml_edit::Value::Array(arr) => {
            for elem in arr.iter() {
                let Some(span) = elem.span() else { continue };
                if !span.contains(&offset) {
                    continue;
                }
                if let toml_edit::Value::InlineTable(table) = elem
                    && let Some(found) = inline_member(table, path, offset)
                {
                    return Some(found);
                }
                return None;
            }
            None
        }
        _ => None,
    }
}

fn inline_member(table: &toml_edit::InlineTable, path: &str, offset: usize) -> Option<CursorField> {
    for (key, val) in table.iter() {
        let member = child_path(path, key);
        if let Some(span) = table.key(key).and_then(|k| k.span())
            && span.contains(&offset)
        {
            return Some(CursorField::named(member, span.clone()));
        }
        let Some(span) = val.span() else { continue };
        if !span.contains(&offset) {
            continue;
        }
        return Some(match member_in(val, &member, offset) {
            Some(field) => field.within(member),
            None => CursorField::named(member, span.clone()),
        });
    }
    None
}

fn value_on_its_line<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
) -> Option<(Range<usize>, String)> {
    let text = lines.text();
    let line_range = line_range_at(text, offset);
    let line = &text[line_range.clone()];
    let (name, val) = line.split_once('=')?;
    let name = name.trim();
    let key_start = line_range.start + line.find(name)?;
    let (_, path) = key_path(lines, key_start)?;
    let val = val.trim();
    let equals = line.find('=')?;
    let val_start = line_range.start + equals + 1 + line[equals + 1..].find(val)?;
    Some((val_start..val_start + val.len(), path))
}

pub(crate) fn replacement(text: &str, position: Position, host: HostSections) -> Option<TextEdit> {
    let (lines, offset) = at_offset(text, position)?;
    let (range, path) = key_path(&lines, offset)?;
    let closest = corrected(host, &path)?;
    let leaf = text[range.clone()].rsplit('.').next()?;
    let edit = TextEdit {
        range: position::utf16_range(&lines, range.end - leaf.len()..range.end)?,
        new_text: closest,
    };
    valid_edits(&lines, std::slice::from_ref(&edit)).then_some(edit)
}

pub(crate) fn corrected(host: HostSections, path: &str) -> Option<String> {
    field_replacement(path, declared_paths(host).map(|(field, _)| field)).map(str::to_owned)
}

pub(crate) fn valid_edits<T: AsRef<str>>(lines: &Lines<T>, edits: &[TextEdit]) -> bool {
    let mut patches = Vec::with_capacity(edits.len());
    for edit in edits {
        let Some(range) = position::byte_range(lines, edit.range) else {
            return false;
        };
        patches.push((range, &edit.new_text));
    }
    patches.sort_by_key(|(range, _)| range.start);
    if patches.windows(2).any(|p| p[0].0.end > p[1].0.start) {
        return false;
    }
    let mut text = lines.text().to_owned();
    for (range, val) in patches.into_iter().rev() {
        text.replace_range(range, val);
    }
    toml_edit::Document::parse(text).is_ok()
}

pub(crate) fn key_path<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
) -> Option<(Range<usize>, String)> {
    let text = lines.text();
    let (key_range, key) = key_at(text, offset)?;
    // A `[` before the key distinguishes a table header from an array element opening with `[`.
    if text[..key_range.start].trim_end().ends_with('[') {
        return Some((key_range, key.to_owned()));
    }
    let doc = parsed_with_blanked(text, line_range_at(text, offset))?;
    let (_, table_path) = enclosing_table(doc.as_table(), offset)?;
    Some((key_range, child_path(&table_path, key)))
}

fn key_at(text: &str, offset: usize) -> Option<(Range<usize>, &str)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let is_key = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let line_range = line_range_at(text, offset);
    let line = &text[line_range.clone()];
    let col = offset - line_range.start;
    if line[..col].contains(['=', '#']) {
        return None;
    }
    let start = line
        .char_indices()
        .take_while(|(idx, _)| *idx < col)
        .filter(|(_, c)| !is_key(*c))
        .map(|(idx, c)| idx + c.len_utf8())
        .last()
        .unwrap_or(0);
    let end = line
        .char_indices()
        .find(|(idx, c)| *idx >= col && !is_key(*c))
        .map_or(line.len(), |(idx, _)| idx);
    let key = &line[start..end];
    (!key.is_empty()).then_some((line_range.start + start..line_range.start + end, key))
}

fn completion<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
    host: HostSections,
) -> Vec<CompletionItem> {
    let text = lines.text();
    let Some((key_range, prefix)) = key_position(text, offset) else {
        return value_completions(lines, offset, host);
    };
    let Some(doc) = parsed_with_blanked(text, key_range.clone()) else {
        return Vec::new();
    };
    let Some((table, table_path)) = enclosing_table(doc.as_table(), offset) else {
        return Vec::new();
    };
    let Some(range) = position::utf16_range(lines, key_range) else {
        return Vec::new();
    };
    declared_paths(host)
        .filter_map(|(path, _)| {
            let key = immediate_key(path, &table_path)?;
            (key.starts_with(prefix) && !table.contains_key(key)).then(|| CompletionItem {
                label: key.to_owned(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(path.to_owned()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: key.to_owned(),
                })),
                ..CompletionItem::default()
            })
        })
        .collect()
}

fn value_completions<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
    host: HostSections,
) -> Vec<CompletionItem> {
    let text = lines.text();
    let Some(field) = at_field(lines, offset) else {
        return Vec::new();
    };
    let val_range = field.range;
    let Some(declared_path) = field.paths.into_iter().find(|p| declares(host, p)) else {
        return Vec::new();
    };
    let Some(accepted) = field_values(&declared_path) else {
        return Vec::new();
    };
    let typed_range = val_range.start..offset.min(val_range.end);
    let Some(range) = position::utf16_range(lines, typed_range.clone()) else {
        return Vec::new();
    };
    let typed = &text[typed_range];
    accepted
        .iter()
        .filter(|cand| cand.starts_with(typed))
        .map(|cand| CompletionItem {
            label: (*cand).to_owned(),
            kind: Some(CompletionItemKind::VALUE),
            detail: Some(declared_path.clone()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: (*cand).to_owned(),
            })),
            ..CompletionItem::default()
        })
        .collect()
}

fn key_position(text: &str, offset: usize) -> Option<(Range<usize>, &str)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = line_range_at(text, offset).start;
    let prefix = &text[line_start..offset];
    if prefix.contains(['[', ']', '=', '#']) {
        return None;
    }
    let start = line_start + prefix.len() - prefix.trim_start().len();
    let partial = &text[start..offset];
    partial
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        .then_some((start..offset, partial))
}

struct EnclosingTable<'a> {
    table: &'a toml_edit::Table,
    path: String,
    start: usize,
}

/// The latest table header before the cursor owns trailing blank lines even across interleaved
/// array entries; values own their own spans.
fn enclosing_table(root: &toml_edit::Table, offset: usize) -> Option<(&toml_edit::Table, String)> {
    fn walk<'a>(
        table: &'a toml_edit::Table,
        path: &mut String,
        offset: usize,
        owner: &mut EnclosingTable<'a>,
    ) -> Option<()> {
        if let Some(span) = table.span()
            && !table.is_implicit()
            && span.start <= offset
            && span.start >= owner.start
        {
            owner.table = table;
            owner.path.clone_from(path);
            owner.start = span.start;
        }
        for (key, item) in table.iter() {
            match item {
                toml_edit::Item::Value(val) => {
                    if val.span().is_some_and(|span| span.contains(&offset)) {
                        return None;
                    }
                }
                toml_edit::Item::Table(inner) => {
                    with_key(path, key, |path| walk(inner, path, offset, owner))?;
                }
                toml_edit::Item::ArrayOfTables(tables) => with_key(path, key, |path| {
                    for table in tables {
                        walk(table, path, offset, owner)?;
                    }
                    Some(())
                })?,
                toml_edit::Item::None => {}
            }
        }
        Some(())
    }

    let mut owner = EnclosingTable {
        table: root,
        path: String::new(),
        start: 0,
    };
    walk(root, &mut String::new(), offset, &mut owner)?;
    Some((owner.table, owner.path))
}

fn with_key<R>(path: &mut String, key: &str, visit: impl FnOnce(&mut String) -> R) -> R {
    let parent = path.len();
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(key);
    let res = visit(path);
    path.truncate(parent);
    res
}

fn child_path(table_path: &str, key: &str) -> String {
    if table_path.is_empty() {
        key.to_owned()
    } else {
        format!("{table_path}.{key}")
    }
}
