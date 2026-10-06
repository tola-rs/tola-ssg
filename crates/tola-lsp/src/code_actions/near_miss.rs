//! The replacement actions a report's near miss justifies.
//!
//! A key the schema almost spells and a name the standard library holds are corrected from the
//! document's own text, and a key the author did not mean can be removed with the line that
//! writes it.

use lsp_types::{Position, Range, TextEdit};
use tola_build::diagnostic::{SourcePosition, SourceRange, UnknownField};
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::SourceNames;

use crate::server::HostSections;

use super::edits::{byte_range, protocol_line};

/// The fixes one unknown-configuration-field report justifies: the declared key that replaces the
/// misspelling, and the removal of the line that writes the unknown key.
pub(super) fn configuration_fixes(
    source: &Source,
    host: HostSections,
    fields: &[UnknownField],
) -> Vec<(String, Vec<TextEdit>)> {
    let text = source.text();
    let mut fixes = Vec::new();
    for field in fields {
        // The configuration reader counts lines by `\n` alone, so its positions are
        // read through that model rather than the protocol's own.
        let offset = field
            .written
            .and_then(|range| config_byte_offset(text, range.start))
            .or_else(|| {
                let start = config_line_start(text, field.line?)?;
                text[start..]
                    .char_indices()
                    .find(|(_, character)| !character.is_whitespace() && *character != '[')
                    .map(|(column, _)| start + column)
            });
        let Some((bytes, path)) =
            offset.and_then(|offset| crate::query::config::key_path(source.lines(), offset))
        else {
            continue;
        };
        let written = field.written.and_then(|range| config_bytes(text, range));
        // A table header writes its leaf alone, so the span the reader reports for a
        // dotted key is the path's tail: it must end where the path does and spell the
        // key's own name there.
        if path != field.name
            || written.as_ref().is_some_and(|written| {
                written.start < bytes.start
                    || written.end != bytes.end
                    || text.get(written.clone()) != field.name.rsplit('.').next()
            })
            || field
                .line
                .is_some_and(|line| config_line(text, bytes.start) != line)
        {
            continue;
        }
        if let Some(written) = &written
            && let Some(range) = crate::position::utf16_range(source.lines(), written.clone())
            && let Some(suggestion) = crate::query::config::corrected(host, &field.name)
        {
            fixes.push((
                format!("change `{}` to `{suggestion}`", field.name),
                vec![TextEdit {
                    range,
                    new_text: suggestion,
                }],
            ));
        }
        if field.line.is_some()
            && let Some(range) = protocol_line(text, bytes.start)
                .and_then(|line| crate::position::utf16_range(source.lines(), line))
        {
            fixes.push((
                format!("remove `{}`", field.name),
                vec![TextEdit {
                    range,
                    new_text: String::new(),
                }],
            ));
        }
    }
    fixes
}

/// The fixes one unknown-variable report justifies: reading the name under `std` when the standard
/// library holds it, and defining it at the file's start otherwise.
///
/// The report is trusted only while the text it echoed still spells an undeclared name there.
pub(super) fn unknown_variable_fixes(
    source: &Source,
    source_names: Option<&SourceNames>,
    parsed_names: &mut Option<SourceNames>,
    reported: Range,
    name: &str,
) -> Vec<(String, Vec<TextEdit>)> {
    let Some(bytes) = byte_range(source, reported) else {
        return Vec::new();
    };
    if !tola_typst_syntax::typst_syntax::is_ident(name)
        || source.text().get(bytes.clone()) != Some(name)
    {
        return Vec::new();
    }
    let names = match source_names {
        Some(names) => names,
        None => parsed_names.get_or_insert_with(|| SourceNames::new(source.clone())),
    };
    if names.occurrence(bytes.start).is_none_or(|occurrence| {
        occurrence.range != bytes
            || !matches!(
                occurrence.kind,
                tola_typst_syntax::names::OccurrenceKind::Name
            )
    }) || names.declared_at(bytes.start).is_some()
    {
        return Vec::new();
    }
    let mut fixes = Vec::new();
    if tola_typst::world::library::GLOBAL_LIBRARY
        .global
        .scope()
        .get(name)
        .is_some()
    {
        fixes.push((
            format!("use `std.{name}`"),
            vec![TextEdit {
                range: reported,
                new_text: format!("std.{name}"),
            }],
        ));
    }
    fixes.push((
        format!("define `{name}`"),
        vec![TextEdit {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            new_text: format!("#let {name} = none\n"),
        }],
    ));
    fixes
}

/// The byte range one configuration span addresses, in the model the configuration reader uses:
/// lines break on `\n` alone, and a column counts UTF-16 units from that line's start.
fn config_bytes(text: &str, range: SourceRange) -> Option<std::ops::Range<usize>> {
    let start = config_byte_offset(text, range.start)?;
    let end = config_byte_offset(text, range.end)?;
    (start <= end).then_some(start..end)
}

/// The byte offset one configuration position addresses.
fn config_byte_offset(text: &str, at: SourcePosition) -> Option<usize> {
    let mut start = 0;
    for _ in 0..at.line {
        let newline = text[start..].find('\n')?;
        start += newline + 1;
    }
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |newline| start + newline);
    let mut utf16 = 0;
    for (offset, character) in text[start..end].char_indices() {
        if utf16 == at.character {
            return Some(start + offset);
        }
        utf16 += character.len_utf16();
        if utf16 > at.character {
            return None;
        }
    }
    (utf16 == at.character).then_some(end)
}

/// The one-based line the configuration reader counts for `byte`.
fn config_line(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

/// The byte offset one one-based line of the configuration reader's model starts at.
fn config_line_start(text: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    let mut start = 0;
    for _ in 1..line {
        let newline = text[start..].find('\n')?;
        start += newline + 1;
    }
    (start <= text.len()).then_some(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::tests::{actions, actions_at, edits, field};
    use lsp_types::{CodeActionOrCommand, Position};

    #[test]
    fn misspelled_field_changes_to_the_declared_key() {
        let text = "[site]\ntitle = \"Editor contract\"\ndescriptio = \"Typo\"\n";
        let actions = actions(
            text,
            serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [{
                    "name": "site.descriptio",
                    "line": 3,
                    "written": {
                        "start": { "line": 2, "character": 0 },
                        "end": { "line": 2, "character": 10 },
                    },
                }],
            }),
        );
        // The correction leads, so a client that takes the first fix keeps the value beside the key.
        assert_eq!(
            edits(&actions),
            [
                (
                    "change `site.descriptio` to `description`".to_owned(),
                    Range::new(Position::new(2, 0), Position::new(2, 10)),
                    "description".to_owned(),
                ),
                (
                    "remove `site.descriptio`".to_owned(),
                    Range::new(Position::new(2, 0), Position::new(3, 0)),
                    String::new(),
                ),
            ]
        );
    }

    #[test]
    fn unknown_field_removes_its_written_line() {
        let text = "version = \"0.8.0\"\n\n[build]\nentry = \"site.typ\"\n\n[build.invalid-section]\nunknown = 1\n";
        let actions = actions(
            text,
            serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [field("build.invalid-section", Some(6))],
            }),
        );
        assert_eq!(
            edits(&actions),
            [(
                "remove `build.invalid-section`".to_owned(),
                Range::new(Position::new(5, 0), Position::new(6, 0)),
                String::new(),
            )]
        );
    }

    #[test]
    fn last_line_removal_stops_at_text_end() {
        let text = "[build]\nentry = \"site.typ\"";
        let actions = actions(
            text,
            serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [field("build.entry", Some(2))],
            }),
        );
        assert_eq!(
            edits(&actions),
            [(
                "remove `build.entry`".to_owned(),
                Range::new(Position::new(1, 0), Position::new(1, 18)),
                String::new(),
            )]
        );
    }

    #[test]
    fn line_beyond_document_offers_no_fix() {
        let actions = actions(
            "[build]\n",
            serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [field("build.minfy", Some(9))],
            }),
        );
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn stale_configuration_echo_preserves_key() {
        let actions = actions(
            "[site]\ndescription = \"kept\"\n",
            serde_json::json!({
                "kind":"unknown-configuration-fields",
                "fields":[{"name":"site.descriptio","line":2,"written":{
                    "start":{"line":1,"character":0},"end":{"line":1,"character":10}
                }}]
            }),
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn unknown_variable_defines_name_at_file_start() {
        let actions = actions_at(
            "#let missing = absent-name\n\n#missing\n",
            serde_json::json!({"kind": "unknown-variable", "name": "absent-name"}),
            Range::new(Position::new(0, 15), Position::new(0, 26)),
        );
        assert_eq!(
            edits(&actions),
            [(
                "define `absent-name`".to_owned(),
                Range::new(Position::new(0, 0), Position::new(0, 0)),
                "#let absent-name = none\n".to_owned(),
            )]
        );
    }

    #[test]
    fn name_outside_language_identifiers_offers_no_fix() {
        for name in ["1x", "a b", "", "1"] {
            let actions = actions(
                "#let x = 1\n",
                serde_json::json!({"kind": "unknown-variable", "name": name}),
            );
            assert!(actions.is_empty(), "{name:?} offered {actions:?}");
        }
    }

    #[test]
    fn std_module_name_rewrites_under_std() {
        let actions = actions_at(
            "$ sym.alpha $\n",
            serde_json::json!({"kind": "unknown-variable", "name": "sym"}),
            Range::new(Position::new(0, 2), Position::new(0, 5)),
        );

        // The library's own name leads, because the binding below would shadow it.
        assert_eq!(
            edits(&actions),
            [
                (
                    "use `std.sym`".to_owned(),
                    Range::new(Position::new(0, 2), Position::new(0, 5)),
                    "std.sym".to_owned(),
                ),
                (
                    "define `sym`".to_owned(),
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                    "#let sym = none\n".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn stale_variable_echo_offers_no_edit() {
        for text in ["#let sym = 1\n", "//sym", "#\"sym\""] {
            let actions = actions_at(
                text,
                serde_json::json!({"kind":"unknown-variable", "name":"sym"}),
                Range::new(Position::new(0, 2), Position::new(0, 5)),
            );
            assert!(actions.is_empty(), "{text:?} offered {actions:?}");
        }
    }

    #[test]
    fn alternative_fixes_are_not_preferred() {
        let actions = actions_at(
            "$ sym.alpha $\n",
            serde_json::json!({"kind":"unknown-variable", "name":"sym"}),
            Range::new(Position::new(0, 2), Position::new(0, 5)),
        );
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|action| matches!(action,
            CodeActionOrCommand::CodeAction(action) if action.is_preferred != Some(true))));
    }
}
