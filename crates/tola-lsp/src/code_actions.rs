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
use tola_build::diagnostic::DiagnosticCause;
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::SourceNames;

use crate::server::HostSections;

use self::edits::{byte_range, disjoint_edits, edit_action, merged_removals};
use self::headings::heading_depth;
use self::imports::{organize_imports, unread_import_removal};
use self::near_miss::{configuration_fixes, unknown_variable_fixes};
use self::wrapping::{equation_rewrites, figure_wrap, wrap_content_block};

pub(super) use self::narrowing::{NarrowingSource, narrowing_actions, narrowing_closure};

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
    let mut fixes: Vec<(String, Vec<TextEdit>)> = Vec::new();
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
                fixes.extend(unknown_variable_fixes(
                    source,
                    source_names,
                    &mut parsed_names,
                    diagnostic.range,
                    &name,
                ));
            }
            DiagnosticCause::UnreadImport { names, removal } => {
                let names_index = source_names.unwrap_or_else(|| {
                    parsed_names.get_or_insert_with(|| SourceNames::new(source.clone()))
                });
                let Some(edits) = unread_import_removal(source, names_index, &names, &removal)
                else {
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
                    fixes.push((title, edits));
                }
            }
            _ => {}
        }
    }
    let fix_all_edits = fix_all.then(|| {
        disjoint_edits(
            fixes
                .iter()
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
                .filter(|(_, edits)| {
                    seen.insert(
                        edits
                            .iter()
                            .map(|edit| (edit.range.start, edit.range.end, edit.new_text.clone()))
                            .collect::<Vec<_>>(),
                    )
                })
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
    let Some(bytes) = byte_range(source, span) else {
        return Vec::new();
    };
    let mut actions = heading_depth(uri, source, bytes.clone());
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
    use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, NumberOrString};

    /// The document every request names: absolute on any host, and no file needs to exist.
    pub(super) fn uri() -> Uri {
        let root = std::env::current_dir().expect("the tests run in the crate's directory");
        crate::uri::from_file_path(&root.join("site/tola.toml")).expect("a file uri")
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
            Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
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
            Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            Some(only),
        )
    }

    /// One request for `text` that echoes `causes` without naming any kind.
    pub(super) fn actions_unfiltered(
        text: &str,
        causes: Vec<serde_json::Value>,
    ) -> Vec<CodeActionOrCommand> {
        actions_over(
            text,
            causes,
            Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            None,
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
        let params = CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri() },
            range: span,
            context: lsp_types::CodeActionContext {
                diagnostics: causes
                    .into_iter()
                    .map(|data| Diagnostic {
                        range: span,
                        data: Some(data),
                        ..Diagnostic::default()
                    })
                    .collect(),
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
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
            .map(|(range, new_text)| (byte_range(&source, range).expect("an edit range"), new_text))
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

    /// The titled edits every action of `kind` has.
    pub(super) fn edits_of_kind(
        actions: &[CodeActionOrCommand],
        kind: &CodeActionKind,
    ) -> Vec<(String, Range, String)> {
        actions
            .iter()
            .map(|action| {
                let CodeActionOrCommand::CodeAction(CodeAction {
                    title,
                    kind: action_kind,
                    edit: Some(workspace),
                    ..
                }) = action
                else {
                    panic!("every action is a code action");
                };
                assert_eq!(action_kind.as_ref(), Some(kind));
                let edit = workspace
                    .changes
                    .as_ref()
                    .and_then(|changes| changes.get(&uri()))
                    .and_then(|edits| edits.first())
                    .expect("every action has one edit");
                (title.clone(), edit.range, edit.new_text.clone())
            })
            .collect()
    }

    pub(super) fn edits(actions: &[CodeActionOrCommand]) -> Vec<(String, Range, String)> {
        edits_of_kind(actions, &CodeActionKind::QUICKFIX)
    }

    #[test]
    fn diagnostic_without_cause_offers_no_fix() {
        let params = CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri() },
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            context: lsp_types::CodeActionContext {
                diagnostics: vec![Diagnostic {
                    range: Range::new(
                        lsp_types::Position::new(0, 0),
                        lsp_types::Position::new(0, 0),
                    ),
                    code: Some(NumberOrString::String("typst.compile".to_owned())),
                    message: "unknown variable: x".to_owned(),
                    ..Diagnostic::default()
                }],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        assert!(
            diagnostic_actions(
                &uri(),
                &Source::detached("#let x = 1\n"),
                None,
                &params,
                &[]
            )
            .is_empty()
        );
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
                Range::new(
                    lsp_types::Position::new(2, 0),
                    lsp_types::Position::new(3, 0)
                ),
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

        for (asked, wanted, refused) in [
            (
                CodeActionKind::QUICKFIX,
                "remove the unused import of `sitemap`",
                "organize imports",
            ),
            (
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                "organize imports",
                "remove the unused import of `sitemap`",
            ),
        ] {
            let actions = actions_for(text, vec![cause.clone()], vec![asked.clone()]);
            let titles = || {
                actions
                    .iter()
                    .filter_map(|action| match action {
                        CodeActionOrCommand::CodeAction(action) => Some(action.title.as_str()),
                        CodeActionOrCommand::Command(_) => None,
                    })
                    .collect::<Vec<_>>()
            };
            assert!(titles().contains(&wanted), "{asked:?} gave {actions:?}");
            assert!(!titles().contains(&refused), "{asked:?} gave {actions:?}");
        }
    }

    #[test]
    fn parent_kind_admits_its_children() {
        assert!(admits(None, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
        assert!(admits(
            Some(&[CodeActionKind::new("source")]),
            &CodeActionKind::SOURCE_ORGANIZE_IMPORTS
        ));
        assert!(admits(
            Some(&[CodeActionKind::new("refactor")]),
            &CodeActionKind::REFACTOR_REWRITE
        ));
        assert!(!admits(
            Some(&[CodeActionKind::QUICKFIX]),
            &CodeActionKind::SOURCE_ORGANIZE_IMPORTS
        ));
    }

    #[test]
    fn fix_all_applies_every_diagnostic_fix() {
        let text = "[site]\ntitel = \"Site\"\ndescriptio = \"Typo\"\n";
        let actions = actions_for(
            text,
            vec![
                serde_json::json!({
                    "kind": "unknown-configuration-fields",
                    "fields": [{
                        "name": "site.titel",
                        "line": 2,
                        "written": {
                            "start": {"line": 1, "character": 0},
                            "end": {"line": 1, "character": 5},
                        },
                    }],
                }),
                serde_json::json!({
                    "kind": "unknown-configuration-fields",
                    "fields": [{
                        "name": "site.descriptio",
                        "line": 3,
                        "written": {
                            "start": {"line": 2, "character": 0},
                            "end": {"line": 2, "character": 10},
                        },
                    }],
                }),
            ],
            vec![CodeActionKind::SOURCE_FIX_ALL],
        );

        // One action answers both reports, and each field's removal yields to its correction.
        assert_eq!(
            action_kinds(&actions),
            [Some(CodeActionKind::SOURCE_FIX_ALL)]
        );
        assert_eq!(
            edits_of(&actions, "fix every diagnostic in this file"),
            [
                (
                    Range::new(
                        lsp_types::Position::new(1, 0),
                        lsp_types::Position::new(1, 5)
                    ),
                    "title".to_owned(),
                ),
                (
                    Range::new(
                        lsp_types::Position::new(2, 0),
                        lsp_types::Position::new(2, 10)
                    ),
                    "description".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn unfixable_diagnostic_answers_no_fix_all() {
        for causes in [
            Vec::new(),
            vec![serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [field("build.minfy", Some(9))],
            })],
        ] {
            let actions = actions_for("[build]\n", causes, vec![CodeActionKind::SOURCE_FIX_ALL]);
            assert!(actions.is_empty(), "{actions:?}");
        }
    }

    #[test]
    fn unfiltered_request_omits_fix_all() {
        let text = "[site]\ntitle = \"Site\"\ndescriptio = \"Typo\"\n";
        let actions = actions_unfiltered(
            text,
            vec![serde_json::json!({
                "kind": "unknown-configuration-fields",
                "fields": [{
                    "name": "site.descriptio",
                    "line": 3,
                    "written": {
                        "start": {"line": 2, "character": 0},
                        "end": {"line": 2, "character": 10},
                    },
                }],
            })],
        );

        // The request still earns its quick fixes; only the fix-all action stays out.
        assert!(!edits(&actions).is_empty(), "{actions:?}");
        assert!(edits_of(&actions, "fix every diagnostic in this file").is_empty());
        assert!(!action_kinds(&actions).contains(&Some(CodeActionKind::SOURCE_FIX_ALL)));
    }
}
