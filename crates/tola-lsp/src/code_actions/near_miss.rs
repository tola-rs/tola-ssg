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
            && let Some(line) = protocol_line(text, bytes.start)
            && !writes_a_table_header(&text[line.clone()])
            && let Some(range) = crate::position::utf16_range(source.lines(), line)
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

/// Whether the line a key was reported on writes a table header, whose removal would leave every
/// key below it in whichever table precedes it.
fn writes_a_table_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::tests::{actions, actions_at, edits, field};
    use lsp_types::Position;
    /// A correction is offered only where the echoed cause still describes the text: a header
    /// line, an out-of-range line, a stale echo, and a name the language cannot declare all offer
    /// nothing, so a client never applies an edit to a document it no longer holds.
    #[test]
    fn stale_or_unaddressable_causes_offer_no_fix() {
        // Rows: (document, echoed cause, requested position, why no fix applies).
        let cases: [(&str, serde_json::Value, Option<Range>, &str); 6] = [
            (
                "version = \"0.8.0\"\n\n[build]\nentry = \"site.typ\"\n\n[build.invalid-section]\nunknown = 1\n",
                serde_json::json!({
                    "kind": "unknown-configuration-fields",
                    "fields": [field("build.invalid-section", Some(6))],
                }),
                None,
                "a table header line cannot be removed alone",
            ),
            (
                "[build]\n",
                serde_json::json!({
                    "kind": "unknown-configuration-fields",
                    "fields": [field("build.minfy", Some(9))],
                }),
                None,
                "the echoed line lies past the document",
            ),
            (
                "[site]\ndescription = \"kept\"\n",
                serde_json::json!({
                    "kind": "unknown-configuration-fields",
                    "fields": [{"name": "site.descriptio", "line": 2, "written": {
                        "start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 10},
                    }}],
                }),
                None,
                "the echo names a key the document does not hold",
            ),
            (
                "#let x = 1\n",
                serde_json::json!({"kind": "unknown-variable", "name": "1x"}),
                None,
                "the name is not an identifier",
            ),
            (
                "#let x = 1\n",
                serde_json::json!({"kind": "unknown-variable", "name": "a b"}),
                None,
                "the name is not an identifier",
            ),
            (
                "#let sym = 1\n",
                serde_json::json!({"kind": "unknown-variable", "name": "sym"}),
                Some(Range::new(Position::new(0, 2), Position::new(0, 5))),
                "the echoed name is already declared",
            ),
        ];
        for (text, cause, span, why) in cases {
            let actions = match span {
                Some(span) => actions_at(text, cause, span),
                None => actions(text, cause),
            };
            assert!(actions.is_empty(), "{why}: {actions:?}");
        }
    }
    #[test]
    fn unknown_variable_defines_at_start() {
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
}
