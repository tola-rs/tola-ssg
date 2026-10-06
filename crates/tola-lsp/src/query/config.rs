//! Configuration key completion and hover for the site's `tola.toml`.
//!
//! Every key comes from the schema's own declaration, so a field, a table header, or a value the
//! configuration accepts answers without a second list to keep in step.

use std::ops::Range;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Hover, Position, TextEdit,
};
use tola_build::config::{field_documentation, field_paths, field_values};
use tola_typst::typst::syntax::Lines;

use crate::position;
use crate::server::HostSections;

/// The completions for the site's configuration document at `position`.
///
/// The text comes from the editor, so an unsaved edit completes like a saved one, and the
/// document needs no site identity: positions and keys are the text's own.
pub(crate) fn document(text: &str, position: Position, host: HostSections) -> Vec<CompletionItem> {
    let lines = Lines::new(text.to_owned());
    let Ok(offset) = position::byte_offset(&lines, position) else {
        return Vec::new();
    };
    completion(&lines, offset, host)
}

/// The hover an author receives over a configuration key, a table header, or the value a key
/// has.
///
/// The innermost declared path the cursor reads answers: an author who rests on `html` inside
/// `minify = { html = true }` asks about `build.minify.html`, exactly as one who rests on the key
/// writing it or on the value it has. A member no schema declares answers as the field that
/// has it.
pub(crate) fn document_hover(text: &str, position: Position, host: HostSections) -> Option<Hover> {
    let lines = Lines::new(text.to_owned());
    let offset = position::byte_offset(&lines, position).ok()?;
    let (range, candidates) = at_field(&lines, offset)?;
    let range = position::utf16_range(&lines, range)?;
    let (path, documentation) = candidates
        .into_iter()
        .find_map(|path| documentation(host, &path).map(|documentation| (path, documentation)))?;
    Some(crate::protocol::markdown_hover(
        format!("`{path}`\n\n{documentation}"),
        Some(range),
    ))
}

/// The documentation a declared configuration key has: the core schema's, or a host section's.
fn documentation(host: HostSections, path: &str) -> Option<&'static str> {
    field_documentation(path).or_else(|| {
        host.iter()
            .flat_map(|section| section.iter())
            .find(|(field, _)| field.as_str() == path)
            .and_then(|(_, documentation)| *documentation)
    })
}

/// Whether the core schema or one of the host's sections declares `path`.
fn declares(host: HostSections, path: &str) -> bool {
    field_paths().any(|field| field.as_str() == path)
        || host
            .iter()
            .flat_map(|section| section.iter())
            .any(|(field, _)| field.as_str() == path)
}

/// Every declared configuration key, the core schema's first, then the host's.
fn declared_paths(host: HostSections) -> impl Iterator<Item = &'static str> {
    field_paths().map(|field| field.as_str()).chain(
        host.iter()
            .flat_map(|section| section.iter())
            .map(|(field, _)| field.as_str()),
    )
}

/// The field the cursor reads: the key it names, or the value that key has, with the declared
/// paths from the innermost member outward.
fn at_field<T: AsRef<str>>(lines: &Lines<T>, offset: usize) -> Option<(Range<usize>, Vec<String>)> {
    key_path(lines, offset)
        .map(|(range, path)| (range, vec![path]))
        .or_else(|| value_path(lines, offset))
}

/// The path of the value the cursor reads, and the range the value occupies, with the declared
/// paths from the innermost member outward.
///
/// The document locates the value, so an element of a multi-line array answers like a value beside
/// its key. Text the parser cannot read, or a cursor beside the value rather than inside it, falls
/// back to the line the cursor stands on.
fn value_path<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
) -> Option<(Range<usize>, Vec<String>)> {
    let text = lines.text();
    if let Ok(document) = toml_edit::Document::parse(text)
        && let Some(value) = enclosing_value(document.as_table(), offset)
    {
        return Some(value);
    }
    value_on_its_line(lines, offset).map(|(range, path)| (range, vec![path]))
}

/// The innermost key whose value the cursor reads, as the value's range and the declared paths
/// from the innermost member outward.
///
/// A member of an inline table, and of an inline table inside an array, is a declared path of its
/// own: `minify = { html = true }` answers `build.minify.html` before `build.minify`.
fn enclosing_value(root: &toml_edit::Table, offset: usize) -> Option<(Range<usize>, Vec<String>)> {
    fn walk(
        table: &toml_edit::Table,
        path: &mut String,
        offset: usize,
        found: &mut Option<(Range<usize>, Vec<String>)>,
    ) {
        for (key, item) in table.iter() {
            match item {
                toml_edit::Item::Value(value) => {
                    let Some(span) = value.span() else { continue };
                    if !span.contains(&offset) {
                        continue;
                    }
                    let carrier = child_path(path, key);
                    *found = Some(match member_in(value, &carrier, offset) {
                        Some((range, mut members)) => {
                            members.push(carrier);
                            (range, members)
                        }
                        None => (span.clone(), vec![carrier]),
                    });
                }
                toml_edit::Item::Table(inner) => {
                    let parent_len = path.len();
                    push_key(path, key);
                    walk(inner, path, offset, found);
                    path.truncate(parent_len);
                }
                toml_edit::Item::ArrayOfTables(inner) => {
                    let parent_len = path.len();
                    push_key(path, key);
                    for table in inner {
                        walk(table, path, offset, found);
                    }
                    path.truncate(parent_len);
                }
                toml_edit::Item::None => {}
            }
        }
    }

    let mut found = None;
    walk(root, &mut String::new(), offset, &mut found);
    found
}

/// The member of an inline table, or of an inline table inside an array, that the cursor reads,
/// with the declared paths from that member outward.
///
/// Answers nothing for a scalar: a plain array's element is the array field's own value.
fn member_in(
    value: &toml_edit::Value,
    path: &str,
    offset: usize,
) -> Option<(Range<usize>, Vec<String>)> {
    match value {
        toml_edit::Value::InlineTable(table) => inline_member(table, path, offset),
        toml_edit::Value::Array(array) => {
            for element in array.iter() {
                let Some(span) = element.span() else { continue };
                if !span.contains(&offset) {
                    continue;
                }
                if let toml_edit::Value::InlineTable(table) = element
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

/// The member of an inline table the cursor reads, with the declared paths from that member
/// outward.
fn inline_member(
    table: &toml_edit::InlineTable,
    path: &str,
    offset: usize,
) -> Option<(Range<usize>, Vec<String>)> {
    for (key, value) in table.iter() {
        let member = child_path(path, key);
        if let Some(span) = table.key(key).and_then(|key| key.span())
            && span.contains(&offset)
        {
            return Some((span.clone(), vec![member]));
        }
        let Some(span) = value.span() else { continue };
        if !span.contains(&offset) {
            continue;
        }
        return Some(match member_in(value, &member, offset) {
            Some((range, mut members)) => {
                members.push(member);
                (range, members)
            }
            None => (span.clone(), vec![member]),
        });
    }
    None
}

/// The value the cursor reads on the line it stands on, as the key's path and the value's text.
///
/// Answers for text the parser cannot read yet: the name before the `=` locates the field, and
/// what follows it is the value being written.
fn value_on_its_line<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
) -> Option<(Range<usize>, String)> {
    let text = lines.text();
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    let line = &text[line_start..line_end];
    let (name, value) = line.split_once('=')?;
    let name = name.trim();
    let key_start = line_start + line.find(name)?;
    let (_, path) = key_path(lines, key_start)?;
    let value = value.trim();
    let value_start = line_start + value_offset(line, value)?;
    Some((value_start..value_start + value.len(), path))
}

/// Where `value` begins inside its line, right of the `=` that names it.
fn value_offset(line: &str, value: &str) -> Option<usize> {
    let equals = line.find('=')?;
    Some(equals + 1 + line[equals + 1..].find(value)?)
}

/// The edit that replaces a misspelled key with the declared key it nearly spells.
///
/// A key the schema declares needs no fix, and one far from every declared key is something else
/// the schema does not know, so only a near miss answers an edit.
pub(crate) fn replacement(text: &str, position: Position, host: HostSections) -> Option<TextEdit> {
    let lines = Lines::new(text.to_owned());
    let offset = position::byte_offset(&lines, position).ok()?;
    let (range, path) = key_path(&lines, offset)?;
    let closest = corrected(host, &path)?;
    Some(TextEdit {
        range: position::utf16_range(&lines, range)?,
        new_text: closest,
    })
}

/// The declared key one written configuration path nearly spells.
///
/// The name is the key alone, because the tables leading to it already stand in the document. A
/// key the schema declares needs no correction, and one far from every declared key is something
/// else the schema does not know, so only a near miss answers a name.
pub(crate) fn corrected(host: HostSections, path: &str) -> Option<String> {
    if declares(host, path) {
        return None;
    }
    let (table, written) = match path.rsplit_once('.') {
        Some((table, key)) => (table, key),
        None => ("", path),
    };
    let declared: Vec<String> = declared_paths(host)
        .filter_map(|field| immediate_key(field, table).map(str::to_owned))
        .collect();
    crate::nearest::closest(written, declared.iter().map(String::as_str)).map(str::to_owned)
}

/// The dotted path of the key the cursor names, and the range it occupies.
pub(crate) fn key_path<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
) -> Option<(Range<usize>, String)> {
    let text = lines.text();
    let (key_range, key) = key_at(text, offset)?;
    // A table header names its own section rather than a key of one, and the `[` the key stands
    // behind is what tells a header from an array element that opens its line with one.
    if text[..key_range.start].trim_end().ends_with('[') {
        return Some((key_range, key.to_owned()));
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    // The line is blanked out because a key the author is still writing leaves the document
    // incomplete, and a finished one stands in the way of its own value. Blanking keeps every
    // offset, so the table this finds is the one the cursor writes into.
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    let mut blanked = text.to_owned();
    blanked.replace_range(line_start..line_end, &" ".repeat(line_end - line_start));
    let document = toml_edit::Document::parse(&blanked).ok()?;
    let (_, table_path) = enclosing_table(document.as_table(), offset)?;
    let path = child_path(&table_path, key);
    Some((key_range, path))
}

/// The key the cursor is inside or at the end of, and the range it occupies.
fn key_at(text: &str, offset: usize) -> Option<(Range<usize>, &str)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let name = |character: char| {
        character.is_ascii_alphanumeric()
            || character == '_'
            || character == '-'
            || character == '.'
    };
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    let line = &text[line_start..line_end];
    let column = offset - line_start;
    // A key is named before its `=`; a value or a comment names none.
    if line[..column].contains(['=', '#']) {
        return None;
    }
    let start = line
        .char_indices()
        .take_while(|(index, _)| *index < column)
        .filter(|(_, character)| !name(*character))
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    let end = line
        .char_indices()
        .find(|(index, character)| *index >= column && !name(*character))
        .map_or(line.len(), |(index, _)| index);
    let key = &line[start..end];
    (!key.is_empty()).then_some((line_start + start..line_start + end, key))
}

/// The keys an author may write at `offset`, minus the ones the enclosing table declares.
///
/// Answers nothing outside a key position: a table header, a value, and a comment each name
/// something other than a key, and an inline table's members belong to the value being written.
pub(super) fn completion<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
    host: HostSections,
) -> Vec<CompletionItem> {
    let text = lines.text();
    let Some((key_range, prefix)) = key_position(text, offset) else {
        return value_completions(lines, offset, host);
    };
    // A key the author is still writing leaves the document incomplete, so the parse that
    // locates the table runs on a copy whose partial key is blanked out. Blanking keeps every
    // other offset, so the ranges this answers remain the document's own.
    let mut blanked = text.to_owned();
    blanked.replace_range(key_range.clone(), &" ".repeat(key_range.len()));
    let Ok(document) = toml_edit::Document::parse(&blanked) else {
        return Vec::new();
    };
    let Some((table, table_path)) = enclosing_table(document.as_table(), offset) else {
        return Vec::new();
    };
    let Some(range) = position::utf16_range(lines, key_range) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for path in declared_paths(host) {
        let Some(key) = immediate_key(path, &table_path) else {
            continue;
        };
        if !key.starts_with(prefix) || table.contains_key(key) {
            continue;
        }
        items.push(CompletionItem {
            label: key.to_owned(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(path.to_owned()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: key.to_owned(),
            })),
            ..CompletionItem::default()
        });
    }
    items
}

/// The byte range a completion replaces and the partial key it replaces.
/// The values of the field whose value the cursor writes.
///
/// A field whose type states its values — a boolean takes `true` and `false` — completes them where
/// a key completes its name; a field whose type states none stays unoffered, because its
/// documentation is where its accepted forms live.
fn value_completions<T: AsRef<str>>(
    lines: &Lines<T>,
    offset: usize,
    host: HostSections,
) -> Vec<CompletionItem> {
    let text = lines.text();
    let Some((value, candidates)) = at_field(lines, offset) else {
        return Vec::new();
    };
    let Some(path) = candidates.into_iter().find(|path| declares(host, path)) else {
        return Vec::new();
    };
    let Some(values) = field_values(&path) else {
        return Vec::new();
    };
    let typed = value.start..offset.min(value.end);
    let Some(range) = position::utf16_range(lines, typed.clone()) else {
        return Vec::new();
    };
    let typed = &text[typed];
    values
        .iter()
        .filter(|value| value.starts_with(typed))
        .map(|value| CompletionItem {
            label: (*value).to_owned(),
            kind: Some(CompletionItemKind::VALUE),
            detail: Some(path.clone()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: (*value).to_owned(),
            })),
            ..CompletionItem::default()
        })
        .collect()
}

fn key_position(text: &str, offset: usize) -> Option<(Range<usize>, &str)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let prefix = &text[line_start..offset];
    // `[` opens a table, `=` a value, and `#` a comment; none of them names a key.
    if prefix.contains(['[', ']', '=', '#']) {
        return None;
    }
    let start = line_start + prefix.len() - prefix.trim_start().len();
    let partial = &text[start..offset];
    partial
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
        .then_some((start..offset, partial))
}

/// The latest table header before the cursor owns trailing blank lines, even when
/// an array's entries are interleaved with other sections. Values own their full spans.
fn enclosing_table(root: &toml_edit::Table, offset: usize) -> Option<(&toml_edit::Table, String)> {
    fn walk<'a>(
        table: &'a toml_edit::Table,
        path: &mut String,
        offset: usize,
        owner: &mut (&'a toml_edit::Table, String, usize),
    ) -> Option<()> {
        if let Some(span) = table.span()
            && !table.is_implicit()
            && span.start <= offset
            && span.start >= owner.2
        {
            owner.0 = table;
            owner.1.clone_from(path);
            owner.2 = span.start;
        }
        for (key, item) in table.iter() {
            match item {
                toml_edit::Item::Value(value) => {
                    if value.span().is_some_and(|span| span.contains(&offset)) {
                        return None;
                    }
                }
                toml_edit::Item::Table(_) | toml_edit::Item::ArrayOfTables(_) => {
                    let parent_len = path.len();
                    push_key(path, key);
                    match item {
                        toml_edit::Item::Table(table) => walk(table, path, offset, owner)?,
                        toml_edit::Item::ArrayOfTables(tables) => {
                            for table in tables {
                                walk(table, path, offset, owner)?;
                            }
                        }
                        _ => unreachable!(),
                    }
                    path.truncate(parent_len);
                }
                toml_edit::Item::None => {}
            }
        }
        Some(())
    }

    let mut owner = (root, String::new(), 0);
    walk(root, &mut String::new(), offset, &mut owner)?;
    Some((owner.0, owner.1))
}

/// The key `path` writes directly inside `table_path`, if it writes one at all.
fn immediate_key<'a>(path: &'a str, table_path: &str) -> Option<&'a str> {
    let remainder = if table_path.is_empty() {
        (!path.contains('.')).then_some(path)?
    } else {
        path.strip_prefix(table_path)?.strip_prefix('.')?
    };
    (!remainder.contains('.')).then_some(remainder)
}

/// Extend `path` with one key written below it.
fn push_key(path: &mut String, key: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(key);
}

/// The declared path of `key` written inside `table_path`.
fn child_path(table_path: &str, key: &str) -> String {
    if table_path.is_empty() {
        key.to_owned()
    } else {
        format!("{table_path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::config::FieldPath;

    /// One completion, as the label an author reads, the key it writes, and where it edits.
    struct Completed {
        label: String,
        key: String,
        range: lsp_types::Range,
    }

    /// The completions for `text` at its `|` marker, over a host's own declarations.
    fn completed_in(host: HostSections, text: &str) -> Vec<Completed> {
        let offset = text.find('|').expect("a cursor marker");
        let lines = Lines::new(text.replace('|', ""));
        completion(&lines, offset, host)
            .into_iter()
            .map(|item| {
                let CompletionTextEdit::Edit(edit) = item.text_edit.unwrap() else {
                    panic!("expected a text edit");
                };
                Completed {
                    label: item.label,
                    key: edit.new_text,
                    range: edit.range,
                }
            })
            .collect()
    }

    /// The completions for `text` at its `|` marker, which the completion replaces.
    fn completed(text: &str) -> Vec<Completed> {
        completed_in(&[], text)
    }

    fn labels(text: &str) -> Vec<String> {
        completed(text).into_iter().map(|item| item.label).collect()
    }

    fn labels_in(host: HostSections, text: &str) -> Vec<String> {
        completed_in(host, text)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    /// The markdown the hover at the `|` marker answers with, over a host's own declarations.
    fn hover_in(host: HostSections, text: &str) -> Option<String> {
        let offset = text.find('|').expect("a cursor marker");
        let unmarked = text.replace('|', "");
        let lines = Lines::new(unmarked);
        let position = position::utf16_range(&lines, offset..offset)?.start;
        let hover = document_hover(lines.text(), position, host)?;
        match hover.contents {
            lsp_types::HoverContents::Markup(markup) => Some(markup.value),
            contents => panic!("markdown, got {contents:?}"),
        }
    }

    /// The markdown the hover at the `|` marker answers with, when it answers.
    fn hover(text: &str) -> Option<String> {
        hover_in(&[], text)
    }

    /// The hover markdown a declared path answers with: the path as code, then its documentation.
    fn documented(path: &str) -> String {
        format!(
            "`{path}`\n\n{}",
            field_documentation(path).expect("a declared path")
        )
    }

    /// A host's own declarations, as the server receives them: one section's field list per entry,
    /// each path paired with the documentation its declaring type wrote.
    const HOST: HostSections = &[&[
        (
            FieldPath::new("diagnostics.max_errors"),
            Some("Host-declared errors limit."),
        ),
        (FieldPath::new("build.entry"), Some("Host-declared entry.")),
    ]];

    /// A field only a host section declares answers that declaration's documentation.
    #[test]
    fn host_declared_field_answers_its_documentation() {
        assert_eq!(
            hover_in(HOST, "[diagnostics]\nmax_erro|rs = 3\n"),
            Some("`diagnostics.max_errors`\n\nHost-declared errors limit.".to_owned())
        );
    }

    /// A host declaration of a path the core schema owns leaves the core documentation in place.
    #[test]
    fn core_documentation_wins_over_host() {
        assert_eq!(
            hover_in(HOST, "[build]\nent|ry = \"site.typ\"\n"),
            Some(documented("build.entry"))
        );
    }

    /// A host-declared key completes inside its own section.
    #[test]
    fn host_declared_keys_complete() {
        let offered = labels_in(HOST, "[diagnostics]\n|");
        assert!(offered.contains(&"max_errors".to_owned()), "{offered:?}");
    }

    /// A boolean field completes the two values it takes.
    #[test]
    fn boolean_field_completes_its_values() {
        let labels = labels("[build.minify]\njavascript = tr|\n");
        assert_eq!(labels, ["true"], "{labels:?}");
    }

    /// A value answers the field it belongs to, exactly as its key does.
    #[test]
    fn value_answers_its_field() {
        assert_eq!(
            hover("[site]\nlanguage = \"z|h\"\n"),
            Some(documented("site.language"))
        );
    }

    /// The fix for the key at the `|` marker, with the marker removed from the text.
    fn fix(text: &str) -> Option<TextEdit> {
        let offset = text.find('|').expect("a cursor marker");
        let unmarked = text.replace('|', "");
        let lines = Lines::new(unmarked.clone());
        let position = position::utf16_range(&lines, offset..offset)?.start;
        replacement(&unmarked, position, &[])
    }

    #[test]
    fn misspelled_key_replaced_by_nearest_key() {
        let edit = fix("[build]\nentr| = \"content\"\n").expect("a fix");
        assert_eq!(edit.new_text, "entry");
        assert_eq!(
            (
                edit.range.start.line,
                edit.range.start.character,
                edit.range.end.character
            ),
            (1, 0, 4),
            "the fix replaces the key the author wrote"
        );
    }

    #[test]
    fn only_misspelled_keys_are_fixed() {
        assert!(
            fix("[build]\nent|ry = \"content\"\n").is_none(),
            "a declared key"
        );
        assert!(
            fix("[build]\nentry = \"cont|ent\"\n").is_none(),
            "a value names no key"
        );
        assert!(fix("[bu|ild]\n").is_none(), "a declared table header");
        assert_eq!(
            fix("[bul|d]\n").map(|edit| edit.new_text),
            Some("build".to_owned()),
            "a misspelled table header"
        );
        assert!(
            fix("[site]\nzzzzzz|zz = 1\n").is_none(),
            "a key no declared key resembles"
        );
    }

    #[test]
    fn keys_complete_inside_their_table() {
        let offered = labels("[build]\n|");
        assert!(offered.contains(&"entry".to_owned()), "{offered:?}");
        assert!(offered.contains(&"content-dir".to_owned()), "{offered:?}");
        assert!(!offered.contains(&"title".to_owned()), "{offered:?}");

        let offered = labels("[build]\nent|");
        assert_eq!(offered, ["entry"]);
    }

    #[test]
    fn nested_sections_complete_their_own_keys() {
        let offered = labels("[build.hooks]\n|");
        assert!(offered.contains(&"before-build".to_owned()), "{offered:?}");
        assert!(!offered.contains(&"entry".to_owned()), "{offered:?}");

        assert_eq!(
            labels("|"),
            ["site", "build", "assets", "typst", "icons", "vendor"]
        );
    }

    #[test]
    fn declared_keys_do_not_repeat() {
        let offered = labels("[site]\ntitle = \"Title\"\n|");
        assert!(!offered.contains(&"title".to_owned()), "{offered:?}");
        assert!(offered.contains(&"description".to_owned()), "{offered:?}");
    }

    #[test]
    fn values_and_headers_name_no_key() {
        for text in [
            "[build]\nentry = |\n",
            "version = |\n",
            "[bui|]\n",
            "# |\n",
            "[build]\nentry = \"|\"\n",
        ] {
            assert!(labels(text).is_empty(), "{text:?}");
        }
    }

    /// Every position that names a declared path answers that path: a key, a value, a table
    /// header, a dotted key, an array-of-tables entry, and an inline member.
    #[test]
    fn hover_answers_the_declared_path_at_the_cursor() {
        let cases: [(&str, &str); 12] = [
            ("[build]\nentr|y = \"content\"\n", "build.entry"),
            ("[build.hoo|ks]\n", "build.hooks"),
            ("[site]\nti|tle = \"Title\"\n", "site.title"),
            ("[build]\nminify = { ht|ml = true }\n", "build.minify.html"),
            ("[build]\nminify = { html = tr|ue }\n", "build.minify.html"),
            ("site.ti|tle = \"T\"\n", "site.title"),
            (
                "[build.references]\nnaviga|tion = \"warn\"\n",
                "build.references.navigation",
            ),
            (
                "[[build.hooks.before-build]]\nna|me = \"icons\"\n",
                "build.hooks.before-build.name",
            ),
            ("[site]\nextra = { github = \"u|\" }\n", "site.extra"),
            ("[si|te]\n", "site"),
            (
                "[[build.hooks.before-|build]]\n",
                "build.hooks.before-build",
            ),
            (
                "[[build.hooks.before-build]]\nname = \"icons\"\n\
                 command = [\n  \"node\",\n  \"|scripts/icons.mjs\",\n]\n",
                "build.hooks.before-build.command",
            ),
        ];
        for (source, path) in cases {
            assert_eq!(hover(source), Some(documented(path)), "{source:?}");
        }
    }

    /// An array element that opens its line with `[` is no table header: its text answers as the
    /// array's field, not as a declared path it happens to spell.
    #[test]
    fn array_element_text_answers_its_field() {
        assert_eq!(
            hover("[[build.hooks.before-build]]\ncommand = [\n  [\"s|ite\"],\n]\n"),
            Some(documented("build.hooks.before-build.command"))
        );
    }

    /// Text no declared path describes answers no hover.
    #[test]
    fn undeclared_text_answers_nothing() {
        for source in [
            "|nope = 1\n",
            "[build]\nnope| = 1\n",
            "[bui|]\n",
            "[build]\n|",
            "# |note\n",
        ] {
            assert_eq!(hover(source), None, "{source:?}");
        }
    }

    #[test]
    fn partial_keys_replace_only_themselves() {
        let completed = completed("[build]\nent|");
        let [item] = &completed[..] else {
            panic!("expected one completion");
        };
        assert_eq!(item.key, "entry");
        assert_eq!(item.range.start.line, 1);
        assert_eq!(item.range.start.character, 0);
        assert_eq!(item.range.end.character, 3);
    }

    #[test]
    fn hook_entries_complete_their_fields() {
        for (stage, expected) in [
            (
                "before-build",
                vec!["enable", "name", "command", "dev", "rerun-on", "generates"],
            ),
            (
                "generate-outputs",
                vec!["enable", "name", "command", "dev", "rerun-on", "outputs"],
            ),
            (
                "after-publish",
                vec!["enable", "name", "command", "dev", "rerun-on"],
            ),
        ] {
            assert_eq!(labels(&format!("[[build.hooks.{stage}]]\n|")), expected);
        }
    }

    #[test]
    fn interleaved_hooks_keep_their_fields() {
        let source = r#"[build]
entry = "site.typ"
[[build.hooks.before-build]]
name = "icons"
[site]
title = "Example"
[[build.hooks.after-publish]]
name = "deploy"
[[build.hooks.before-build]]
gen|
"#;
        assert_eq!(labels(source), ["generates"]);
    }

    #[test]
    fn hook_keys_belong_to_each_entry() {
        let source = r#"[[build.hooks.before-build]]
name = "icons"
command = ["icons"]
[[build.hooks.before-build]]
na|
"#;
        assert_eq!(labels(source), ["name"]);
    }

    #[test]
    fn hook_dev_completes_finite_values() {
        for stage in ["before-build", "generate-outputs", "after-publish"] {
            assert_eq!(
                labels(&format!("[[build.hooks.{stage}]]\ndev = |")),
                ["\"run\"", "\"skip\""],
            );
        }
    }
}
