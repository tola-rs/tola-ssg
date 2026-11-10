//! The actions one document's own evidence justifies.
//!
//! The diagnostics a client echoes justify quick fixes, a fix-all pass, and Organize Imports; the
//! selection a client asks about justifies the rewrites that apply to it. Both read current source
//! evidence and structured diagnostics, so they remain available without a successful site
//! compilation and never interpret a rendered diagnostic message.

mod ancestors;
mod edits;
mod headings;
mod imports;
mod narrowing;
mod near_miss;
mod wrapping;

use std::collections::BTreeSet;

use lsp_types::{CodeActionKind, CodeActionOrCommand, CodeActionParams, Range, TextEdit, Uri};
use serde::Deserialize;
use tola_diagnostic::DiagnosticCause;
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::SourceNames;

use crate::server::HostSections;

use self::edits::{disjoint_edits, edit_action, edit_key, merged_removals};
use self::headings::heading_depth;
use self::imports::{organize_imports, unread_import_removal};
use self::near_miss::{configuration_fixes, unknown_variable_fixes};
use self::wrapping::{equation_rewrites, figure_wrap, wrap_content_block};
use crate::position::byte_range;

pub(super) use self::narrowing::{NarrowingSource, narrowing_actions, narrowing_closure};

/// A fix one diagnostic justifies: the title a client shows, and the edits applying it.
pub(super) type Fix = (String, Vec<TextEdit>);

/// The document's name index: the one the request already holds, or one parsed here and kept for
/// the request's later diagnostics.
fn names_index<'a>(
    source: &Source,
    source_names: Option<&'a SourceNames>,
    parsed_names: &'a mut Option<SourceNames>,
) -> &'a SourceNames {
    match source_names {
        Some(source_names) => source_names,
        None => parsed_names.get_or_insert_with(|| SourceNames::new(source.clone())),
    }
}

/// Every fix the request's diagnostics justify for the document they were reported on.
///
/// The diagnostics a client echoes are the reports it asks about, so they are the invocation
/// scope: a client may ask about one from anywhere in the document, and its own range is the
/// cursor it asked from, not a bound on which reports justify a fix.
pub(super) fn diagnostic_actions(
    uri: &Uri,
    source: &Source,
    source_names: Option<&SourceNames>,
    params: &CodeActionParams,
    host: HostSections,
) -> Vec<CodeActionOrCommand> {
    let only = params.context.only.as_deref();
    let quickfix = admits(only, &CodeActionKind::QUICKFIX);
    let organize = admits(only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS);
    let fix_all = only_names(only, &CodeActionKind::SOURCE_FIX_ALL);
    if !quickfix && !organize && !fix_all {
        return Vec::new();
    }
    // The quick-fix and fix-all lanes offer the same fixes, one report at a time and all at
    // once, so either one decides whether the diagnostics are read for their fixes.
    let fixable = quickfix || fix_all;
    let mut parsed_names = None;
    let mut fixes: Vec<Vec<Fix>> = Vec::new();
    let mut organized = Vec::new();
    for diagnostic in &params.context.diagnostics {
        let Some(cause) = &diagnostic.data else {
            continue;
        };
        let Ok(cause) = DiagnosticCause::deserialize(cause) else {
            continue;
        };
        match cause {
            DiagnosticCause::UnknownConfigurationFields { fields } if fixable => {
                fixes.extend(configuration_fixes(source, host, &fields));
            }
            DiagnosticCause::UnknownVariable { name } if fixable => {
                fixes.push(unknown_variable_fixes(
                    source,
                    source_names,
                    &mut parsed_names,
                    diagnostic.range,
                    &name,
                ));
            }
            DiagnosticCause::UnreadImport { names, removal } => {
                let Some(edits) = unread_import_removal(
                    source,
                    names_index(source, source_names, &mut parsed_names),
                    &names,
                    &removal,
                ) else {
                    continue;
                };
                if organize {
                    organized.extend(edits.iter().cloned());
                }
                if fixable {
                    let quoted = names
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let title = if names.len() == 1 {
                        format!("remove the unused import of {quoted}")
                    } else {
                        format!("remove the unused imports of {quoted}")
                    };
                    fixes.push(vec![(title, edits)]);
                }
            }
            _ => {}
        }
    }
    let fix_all_edits = fix_all.then(|| {
        disjoint_edits(
            fixes
                .iter()
                .filter_map(|alternatives| alternatives.first())
                .flat_map(|(_, edits)| edits.iter().cloned())
                .collect(),
        )
    });
    let mut actions = Vec::new();
    if quickfix {
        let mut seen = BTreeSet::new();
        actions.extend(
            fixes
                .into_iter()
                .flatten()
                .filter(|(_, edits)| seen.insert(edits.iter().map(edit_key).collect::<Vec<_>>()))
                .map(|(title, edits)| edit_action(uri, title, CodeActionKind::QUICKFIX, edits)),
        );
    }
    if let Some(edits) = fix_all_edits
        && !edits.is_empty()
    {
        actions.push(edit_action(
            uri,
            "fix every diagnostic in this file".to_owned(),
            CodeActionKind::SOURCE_FIX_ALL,
            edits,
        ));
    }
    if !organized.is_empty() {
        actions.push(organize_imports(uri, merged_removals(organized)));
    }
    actions
}

/// Every refactor the request's selection justifies.
///
/// A refactor reads the syntax the selection covers, so it costs what that selection covers and a
/// request naming a document that is not a source answers nothing.
pub(super) fn selection_actions(
    uri: &Uri,
    source: &Source,
    span: Range,
) -> Vec<CodeActionOrCommand> {
    let Some(bytes) = byte_range(source.lines(), span) else {
        return Vec::new();
    };
    let mut actions = heading_depth(uri, source, &bytes);
    actions.extend(wrap_content_block(uri, source, &bytes));
    actions.extend(equation_rewrites(uri, source, &bytes));
    actions.extend(figure_wrap(uri, source, &bytes));
    actions
}

/// The kinds this lane answers with.
///
/// A kind belongs here once the request filter admits it and a client can ask for it alone;
/// advertising it elsewhere is what makes one of these reachable.
pub(super) const SERVED_KINDS: [CodeActionKind; 4] = [
    CodeActionKind::QUICKFIX,
    CodeActionKind::REFACTOR_REWRITE,
    CodeActionKind::SOURCE_FIX_ALL,
    CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
];

/// Whether a request asking for `only` these kinds admits `kind`.
///
/// An absent or empty filter admits every kind, and the protocol treats a kind as hierarchical,
/// so a request for `source` admits `source.organizeImports`.
pub(super) fn admits(only: Option<&[CodeActionKind]>, kind: &CodeActionKind) -> bool {
    let Some(only) = only.filter(|only| !only.is_empty()) else {
        return true;
    };
    let kind = kind.as_str();
    only.iter().any(|requested| {
        let requested = requested.as_str();
        requested.is_empty()
            || kind == requested
            || kind
                .strip_prefix(requested)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// Whether the request's `only` filter names `kind` rather than admitting every kind.
///
/// An absent or empty filter admits everything, which would answer the fix-all lane for every
/// request and put it in every lightbulb list; only a client that asked for the kind, directly
/// or through a parent kind, is answered.
pub(super) fn only_names(only: Option<&[CodeActionKind]>, kind: &CodeActionKind) -> bool {
    only.is_some_and(|only| !only.is_empty() && admits(Some(only), kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CodeActionKind, CodeActionOrCommand, Diagnostic, Position, Range};

    /// The document every request names: absolute on any host, and no file needs to exist.
    pub(super) fn uri() -> Uri {
        let root = std::env::current_dir().expect("the tests run in the crate's directory");
        crate::uri::from_file_path(&root.join("site/tola.toml")).expect("a file uri")
    }

    /// One code action request for `uri()`, echoing `diagnostics` over `span`.
    fn code_action_params(
        span: Range,
        diagnostics: Vec<Diagnostic>,
        only: Option<Vec<CodeActionKind>>,
    ) -> CodeActionParams {
        CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri() },
            range: span,
            context: lsp_types::CodeActionContext {
                diagnostics,
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    /// One quick fix request for `text`, whose diagnostic covers `span`.
    ///
    /// The request has that range, as a client asking about the report it echoes does.
    pub(super) fn actions_at(
        text: &str,
        cause: serde_json::Value,
        span: Range,
    ) -> Vec<CodeActionOrCommand> {
        actions_over(
            text,
            vec![cause],
            span,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
    }

    pub(super) fn actions(text: &str, cause: serde_json::Value) -> Vec<CodeActionOrCommand> {
        actions_at(
            text,
            cause,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        )
    }

    /// One request for `text` that echoes `causes` and asks for `only` these kinds.
    pub(super) fn actions_for(
        text: &str,
        causes: Vec<serde_json::Value>,
        only: Vec<CodeActionKind>,
    ) -> Vec<CodeActionOrCommand> {
        actions_over(
            text,
            causes,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
            Some(only),
        )
    }
    /// One request for `text` whose echoed diagnostics cover `span`, asking for `only` kinds or
    /// for every kind when the request names none.
    pub(super) fn actions_over(
        text: &str,
        causes: Vec<serde_json::Value>,
        span: Range,
        only: Option<Vec<CodeActionKind>>,
    ) -> Vec<CodeActionOrCommand> {
        let diagnostics = causes
            .into_iter()
            .map(|data| Diagnostic {
                range: span,
                data: Some(data),
                ..Diagnostic::default()
            })
            .collect();
        let params = code_action_params(span, diagnostics, only);
        diagnostic_actions(&uri(), &Source::detached(text), None, &params, &[])
    }

    /// The edits one titled action has, absent when no action has that title.
    pub(super) fn edits_of(actions: &[CodeActionOrCommand], title: &str) -> Vec<(Range, String)> {
        actions
            .iter()
            .find_map(|action| {
                let CodeActionOrCommand::CodeAction(action) = action else {
                    return None;
                };
                (action.title == title).then(|| {
                    action
                        .edit
                        .as_ref()
                        .and_then(|edit| edit.changes.as_ref())
                        .and_then(|changes| changes.get(&uri()))
                        .map(|edits| {
                            edits
                                .iter()
                                .map(|edit| (edit.range, edit.new_text.clone()))
                                .collect::<Vec<_>>()
                        })
                })?
            })
            .unwrap_or_default()
    }

    /// The text one action's edits produce, as an editor applying them reads it.
    pub(super) fn applied(text: &str, edits: Vec<(Range, String)>) -> String {
        let source = Source::detached(text);
        let mut ranges = edits
            .into_iter()
            .map(|(range, new_text)| {
                (
                    byte_range(source.lines(), range).expect("an edit range"),
                    new_text,
                )
            })
            .collect::<Vec<_>>();
        ranges.sort_by_key(|(range, _)| range.start);
        for pair in ranges.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        let mut applied = text.to_owned();
        for (range, new_text) in ranges.into_iter().rev() {
            applied.replace_range(range, &new_text);
        }
        applied
    }

    /// The kind each of `actions` has, absent for a command.
    pub(super) fn action_kinds(actions: &[CodeActionOrCommand]) -> Vec<Option<CodeActionKind>> {
        actions
            .iter()
            .map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => action.kind.clone(),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect()
    }

    pub(super) fn field(name: &str, line: Option<usize>) -> serde_json::Value {
        let mut field = serde_json::json!({ "name": name });
        if let Some(line) = line {
            field["line"] = serde_json::json!(line);
        }
        field
    }

    /// The titled edits every quick fix in `actions` has.
    pub(super) fn edits(actions: &[CodeActionOrCommand]) -> Vec<(String, Range, String)> {
        actions
            .iter()
            .map(|action| {
                let CodeActionOrCommand::CodeAction(action) = action else {
                    panic!("every action is a code action");
                };
                assert_eq!(action.kind.as_ref(), Some(&CodeActionKind::QUICKFIX));
                let edit = action
                    .edit
                    .as_ref()
                    .and_then(|edit| edit.changes.as_ref())
                    .and_then(|changes| changes.get(&uri()))
                    .and_then(|edits| edits.first())
                    .expect("every action has one edit");
                (action.title.clone(), edit.range, edit.new_text.clone())
            })
            .collect()
    }

    #[test]
    fn repeated_unknown_field_yields_one_removal() {
        let text = "[build]\nentry = \"site.typ\"\nminfy = true\n";
        let actions = actions(
            text,
            serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [
                    field("build.minfy", Some(3)),
                    field("build.minfy", Some(3)),
                ],
            }),
        );
        assert_eq!(
            edits(&actions),
            [(
                "remove `build.minfy`".to_owned(),
                Range::new(Position::new(2, 0), Position::new(3, 0)),
                String::new(),
            )]
        );
    }

    #[test]
    fn only_selects_the_kind_it_names() {
        let text = "#import \"@tola/web:0.0.0\": sitemap\n";
        let cause = serde_json::json!({
            "kind": "unread-import",
            "names": ["sitemap"],
            "removal": {"kind": "statement", "line": 1},
        });

        for kind in [
            CodeActionKind::QUICKFIX,
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        ] {
            let asked = kind.clone();
            let actions = actions_for(text, vec![cause.clone()], vec![asked.clone()]);
            assert_eq!(
                action_kinds(&actions),
                [Some(kind)],
                "asking for {asked:?} gave {actions:?}"
            );
        }
    }

    #[test]
    fn fix_all_chooses_one_alternative() {
        let text = "$ sym.alpha $\n";
        let actions = actions_over(
            text,
            vec![serde_json::json!({"kind": "unknown-variable", "name": "sym"})],
            Range::new(Position::new(0, 2), Position::new(0, 5)),
            Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
        );
        assert_eq!(
            applied(
                text,
                edits_of(&actions, "fix every diagnostic in this file")
            ),
            "$ std.sym.alpha $\n"
        );
    }

    #[test]
    fn fix_all_applies_every_diagnostic_fix() {
        let text = "[site]\ntitel = \"Site\"\ndescriptio = \"Typo\"\n";
        let actions = actions_for(
            text,
            vec![serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [{
                    "name": "site.titel",
                    "line": 2,
                    "written": {
                        "start": {"line": 1, "character": 0},
                        "end": {"line": 1, "character": 5},
                    },
                }, {
                    "name": "site.descriptio",
                    "line": 3,
                    "written": {
                        "start": {"line": 2, "character": 0},
                        "end": {"line": 2, "character": 10},
                    },
                }],
            })],
            vec![CodeActionKind::SOURCE_FIX_ALL],
        );

        assert_eq!(
            action_kinds(&actions),
            [Some(CodeActionKind::SOURCE_FIX_ALL)]
        );
        let mut edits = edits_of(&actions, "fix every diagnostic in this file");
        edits.sort_by_key(|(range, _)| range.start);
        assert_eq!(
            edits,
            [
                (
                    Range::new(Position::new(1, 0), Position::new(1, 5)),
                    "title".to_owned(),
                ),
                (
                    Range::new(Position::new(2, 0), Position::new(2, 10)),
                    "description".to_owned(),
                ),
            ]
        );
    }
}
