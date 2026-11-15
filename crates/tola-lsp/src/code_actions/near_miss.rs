//! The replacement actions a report's near miss justifies.
//!
//! A key the schema almost spells and a name the standard library holds are corrected from the
//! document's own text, and a key the author did not mean can be removed with the line that
//! writes it.

use lsp_types::{Position, Range, TextEdit};
use tola_diagnostic::{SourcePosition, UnknownField};
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::SourceNames;

use crate::server::HostSections;

use super::Fix;
use super::edits::{protocol_line, removal};
use crate::position::byte_range;

/// One group per unknown field: its correction first, then removal of the line that writes it.
pub(super) fn configuration_fixes(
    source: &Source,
    host: HostSections,
    fields: &[UnknownField],
    allow_removals: bool,
) -> Vec<Vec<Fix>> {
    let text = source.text();
    let mut fixes = Vec::new();
    for field in fields {
        let Some((bytes, path)) = field_key_offset(source, field)
            .and_then(|offset| crate::query::config::key_path(source.lines(), offset))
        else {
            continue;
        };
        let written = field.written.and_then(|range| {
            let start = config_byte_offset(source, range.start)?;
            let end = config_byte_offset(source, range.end)?;
            (start <= end).then_some(start..end)
        });
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
                .is_some_and(|line| source.lines().byte_to_line(bytes.start) != line.checked_sub(1))
        {
            continue;
        }
        let mut alternatives = Vec::new();
        if let Some(written) = &written
            && let Some(range) = crate::position::utf16_range(source.lines(), written.clone())
            && let Some(suggestion) = crate::query::config::corrected(host, &field.name)
        {
            let title = format!("change `{}` to `{suggestion}`", field.name);
            let edits = vec![TextEdit {
                range,
                new_text: suggestion,
            }];
            if crate::query::config::valid_edits(source.lines(), &edits) {
                alternatives.push((title, edits));
            }
        }
        if allow_removals
            && field.line.is_some()
            && let Some(line) = protocol_line(text, bytes.start)
            && !writes_a_table_header(&text[line.clone()])
            && let Some(range) = crate::position::utf16_range(source.lines(), line)
        {
            let edits = vec![removal(range)];
            if crate::query::config::valid_edits(source.lines(), &edits) {
                alternatives.push((format!("remove `{}`", field.name), edits));
            }
        }
        fixes.push(alternatives);
    }
    fixes
}

/// The fixes one unbound name justifies, with a spelling or library correction first.
///
/// The report is trusted only while the text it echoed still spells an undeclared name there.
pub(super) fn unknown_variable_fixes(
    source: &Source,
    source_names: Option<&SourceNames>,
    parsed_names: &mut Option<SourceNames>,
    reported: Range,
    name: &str,
    allow_definition: bool,
) -> Vec<Fix> {
    let Some(bytes) = byte_range(source.lines(), reported) else {
        return Vec::new();
    };
    let names = super::names_index(source, source_names, parsed_names);
    let Some(occurrence) = crate::lint::names::unbound(names, &bytes, name) else {
        return Vec::new();
    };
    let library = &tola_typst::world::library::GLOBAL_LIBRARY;
    let mut fixes = Vec::new();
    if library.global.scope().get(name).is_some()
        && names
            .resolve("std", occurrence.scope, bytes.start)
            .is_none()
    {
        fixes.push((
            format!("use `std.{name}`"),
            vec![TextEdit {
                range: reported,
                new_text: format!("std.{name}"),
            }],
        ));
    } else if let Some(replacement) = crate::lint::names::replacement(names, occurrence, library) {
        fixes.push((
            format!("change `{name}` to `{replacement}`"),
            vec![TextEdit {
                range: reported,
                new_text: replacement.to_owned(),
            }],
        ));
    }
    if allow_definition {
        fixes.push((
            format!("define `{name}`"),
            vec![TextEdit {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: format!("#let {name} = none\n"),
            }],
        ));
    }
    fixes
}

/// The byte offset the reported field's key starts at.
fn field_key_offset(source: &Source, field: &UnknownField) -> Option<usize> {
    field
        .written
        .and_then(|range| config_byte_offset(source, range.start))
        .or_else(|| {
            let range = source.lines().line_to_range(field.line?.checked_sub(1)?)?;
            source
                .text()
                .get(range.clone())?
                .char_indices()
                .find(|(_, character)| !character.is_whitespace() && *character != '[')
                .map(|(column, _)| range.start + column)
        })
}

/// Read the same compiler-line UTF-16 coordinates the configuration diagnostic wrote.
fn config_byte_offset(source: &Source, at: SourcePosition) -> Option<usize> {
    use tola_typst_syntax::position::{byte_offset, checked_position};
    byte_offset(source.lines(), checked_position(at.line, at.character)?).ok()
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

    /// A correction is offered only while the echoed cause still describes the text.
    #[test]
    fn stale_unaddressable_causes_no_fix() {
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
    fn spelling_fix_uses_only_visible_names() {
        let cause = serde_json::json!({"kind": "unknown-variable", "name": "article-captoin"});
        let range = Range::new(Position::new(1, 1), Position::new(1, 16));
        let visible = actions_at(
            "#let article-caption = [Text]\n#article-captoin\n",
            cause.clone(),
            range,
        );
        assert_eq!(edits(&visible)[0].2, "article-caption");
        let hidden = actions_at(
            "#let section() = { let article-caption = [Text] }\n#article-captoin\n",
            cause,
            range,
        );
        assert!(
            !edits(&hidden)
                .iter()
                .any(|edit| edit.2 == "article-caption")
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
