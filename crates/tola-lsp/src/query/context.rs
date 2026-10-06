//! The request context — the source, the cursor, the construct the cursor stands in, and the
//! revision the query copy realizes — and the source loading one request addresses.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceDiagnosticSession, SourceRevision};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{FileId, Source, SyntaxMode, VirtualRoot};

use crate::compiler::RevisionCompilations;
use crate::protocol::{CheckProgress, SourceQuery};

use super::labels;
use super::repair::{blank_query_name, blank_query_syntax, pin_source};

/// The source one request addresses, or `None` when the site holds no such file.
///
/// A file the editor has not opened and the site does not have answers nothing: an editor asks
/// about any URI it is handed, and "no such source" is a result, not a failure of the request.
pub(super) fn load_source(
    id: FileId,
    config: &ResolvedSiteConfig,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<Source>> {
    let text = match id.root() {
        VirtualRoot::Package(_) => match crate::identity::embedded_source(id) {
            Some(text) => text.into(),
            // A package this implementation does not embed — `@preview`, a local package, a
            // vendored one — is read from the package location the compiler resolved, so its
            // source answers exactly what a builtin package's source answers.
            None => {
                let path = crate::identity::package_file_path(id, config.package_locations())
                    .context("unknown package source")?;
                boundary.check(&path)?;
                match overrides.iter().find(|(open, _)| open == &path) {
                    Some((_, text)) => text.to_string(),
                    None => match std::fs::read_to_string(&path) {
                        Ok(text) => text,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(None);
                        }
                        Err(error) => return Err(error.into()),
                    },
                }
            }
        },
        VirtualRoot::Project => {
            let path = config.get_root().join(id.vpath().get_without_slash());
            boundary.check(&path)?;
            match overrides
                .iter()
                .find(|(path, _)| crate::identity::path_id(path, config.get_root()) == Some(id))
            {
                Some((_, text)) => text.to_string(),
                None => match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error.into()),
                },
            }
        }
    };
    Ok(Some(Source::new(id, text)))
}
/// Selection and edits refer to the original snapshot, not its repaired tree.
pub(super) struct Selection<'a> {
    pub(super) source: &'a Source,
    pub(super) cursor: usize,
    pub(super) syntax: SelectedSyntax,
    pub(super) check_progress: CheckProgress,
}

pub(super) enum SelectedSyntax {
    Expression,
    /// A construct that names something the site does not have yet, which a query copy drops to
    /// compile.
    Name(std::ops::Range<usize>),
    /// A reference at the cursor, which names a label of the site.
    Reference(labels::Reference),
    Import(tola_typst_syntax::syntax::Import),
    ImportedName(tola_typst_syntax::syntax::ImportedName),
    Member(tola_typst_syntax::syntax::Member),
    Call(tola_typst_syntax::syntax::Call),
}
impl<'a> Selection<'a> {
    pub(super) fn new(
        source: &'a Source,
        cursor: usize,
        query: &SourceQuery,
        check_progress: CheckProgress,
    ) -> Self {
        let syntax = match query {
            SourceQuery::Completion(_) => tola_typst_syntax::syntax::import(source, cursor)
                .map(SelectedSyntax::Import)
                .or_else(|| {
                    tola_typst_syntax::syntax::member(source, cursor).map(SelectedSyntax::Member)
                })
                // An unfinished name or label is a syntax error the query copy drops; a finished
                // reference is valid but may still name a label the site does not declare.
                .or_else(|| {
                    tola_typst_syntax::syntax::name(source, cursor).map(SelectedSyntax::Name)
                })
                .or_else(|| {
                    tola_typst_syntax::syntax::include_path(source, cursor)
                        .map(SelectedSyntax::Name)
                })
                .or_else(|| labels::referenced(source, cursor).map(SelectedSyntax::Reference))
                // A path the site cannot read yet is a compile error, so the query copy drops
                // the call that has it while completion answers from the author's text.
                .or_else(|| {
                    tola_typst_syntax::syntax::call(source, cursor).map(SelectedSyntax::Call)
                }),
            SourceQuery::SignatureHelp(_) => {
                tola_typst_syntax::syntax::call(source, cursor).map(SelectedSyntax::Call)
            }
            SourceQuery::PrepareRename(_)
            | SourceQuery::Rename(_)
            | SourceQuery::References(_)
            | SourceQuery::DocumentHighlights(_) => labels::referenced(source, cursor)
                .map(SelectedSyntax::Reference)
                .or_else(|| {
                    tola_typst_syntax::syntax::name(source, cursor).map(SelectedSyntax::Name)
                }),
            // Hints and fixes answer for a viewport: nothing about them depends on the cursor.
            SourceQuery::InlayHints(_)
            | SourceQuery::CodeActions(_)
            | SourceQuery::DocumentColors(_)
            | SourceQuery::ColorPresentations(_) => {
                return Self {
                    source,
                    cursor,
                    syntax: SelectedSyntax::Expression,
                    check_progress,
                };
            }
            // A cursor question at an import item's own name reads the export it names: the name
            // is not a binding in this file, so nothing else resolves it.
            SourceQuery::Hover(_) | SourceQuery::Definition(_) => {
                labels::referenced(source, cursor)
                    .map(SelectedSyntax::Reference)
                    .or_else(|| {
                        tola_typst_syntax::syntax::imported_name(source, cursor)
                            .map(SelectedSyntax::ImportedName)
                    })
            }
        }
        .unwrap_or(SelectedSyntax::Expression);
        Self {
            source,
            cursor,
            syntax,
            check_progress,
        }
    }

    /// Whether the cursor stands at a member position in code, where only the receiver's own
    /// members answer and the general names behind it belong to no member of an unresolved
    /// receiver.
    ///
    /// A member position in math keeps Typst's own completions: a symbol's members answer there.
    pub(super) fn at_code_member(&self) -> bool {
        if !matches!(self.syntax, SelectedSyntax::Member(_)) {
            return false;
        }
        tola_typst_syntax::syntax::cursor_leaves(self.source, self.cursor)
            .into_iter()
            .flatten()
            .any(|leaf| leaf.mode_after() == Some(SyntaxMode::Code))
    }

    /// Whether `range` still holds the author's own bytes in the query copy.
    ///
    /// A range the repair rewrote is a stand-in: an answer about it would describe a fabricated
    /// construct, so no lane answers about it.
    pub(super) fn range_keeps_source_text(
        &self,
        prepared: &Source,
        range: std::ops::Range<usize>,
    ) -> bool {
        prepared.text().as_bytes().get(range.clone()) == self.source.text().as_bytes().get(range)
    }

    /// Whether the construct at the cursor is the author's own text in the query copy.
    ///
    /// A search over the compiled copy — the reader's own — describes what it finds there, so a
    /// construct the repair rewrote has no answer in the author's own text to fall back to.
    pub(super) fn cursor_keeps_source_text(&self, prepared: &Source) -> bool {
        tola_typst_syntax::syntax::expression(prepared, self.cursor)
            .is_none_or(|node| self.range_keeps_source_text(prepared, node.range()))
    }

    pub(super) fn realize(
        &self,
        session: &mut SourceDiagnosticSession,
        compilations: &mut RevisionCompilations,
        config: &Arc<ResolvedSiteConfig>,
        source_revision: u64,
        overrides: &[(PathBuf, Arc<str>)],
        cancellation: &BuildCancellation,
    ) -> Result<Arc<SourceRevision>> {
        let mut prepared = overrides.to_vec();
        if self.source.id().root() == &VirtualRoot::Project {
            pin_source(
                self.source,
                self.source.text().into(),
                config,
                &mut prepared,
            )?;
        }
        let revision =
            compilations.inspect(session, config, source_revision, prepared, cancellation)?;
        if revision.compiled() {
            return Ok(revision);
        }
        // A document the site does not own — an embedded package, which no edit may reach — has only
        // the world it was checked in: a repair writes a query copy of the site's sources, so the
        // unrepaired revision is the answer instead of a failed request.
        if self.source.id().root() != &VirtualRoot::Project {
            return Ok(revision);
        }
        let mut repaired = overrides.to_vec();
        match &self.syntax {
            SelectedSyntax::Import(import) => match &import.repair {
                Some(repair) => {
                    blank_query_syntax(self.source, repair.clone(), config, &mut repaired)?
                }
                None => return Ok(revision),
            },
            SelectedSyntax::Member(member) => {
                blank_query_syntax(self.source, member.suffix.clone(), config, &mut repaired)?
            }
            SelectedSyntax::Call(call) => {
                blank_query_syntax(self.source, call.suffix.clone(), config, &mut repaired)?
            }
            SelectedSyntax::Name(name) => {
                // The author is still typing this name. Completion answers from the file's
                // scopes, not from the name, so the query copy binds it and compiles.
                blank_query_name(self.source, name.clone(), config, &mut repaired)?
            }
            SelectedSyntax::Reference(reference) => {
                // A reference to a label the site does not declare yet is a compile error, so the
                // query copy drops the reference and keeps every offset.
                blank_query_syntax(
                    self.source,
                    reference.written.clone(),
                    config,
                    &mut repaired,
                )?
            }
            SelectedSyntax::Expression | SelectedSyntax::ImportedName(_) => return Ok(revision),
        };
        let repaired =
            compilations.inspect(session, config, source_revision, repaired, cancellation)?;
        if repaired.compiled() {
            return Ok(repaired);
        }
        // A repair can be too narrow: an argument the call rejects, or a call whose own name
        // resolves nowhere. Dropping the call that has the cursor keeps the rest in place.
        let Some(call) = tola_typst_syntax::syntax::call(self.source, self.cursor) else {
            return Ok(repaired);
        };
        let Some(statement) = tola_typst_syntax::syntax::node_at_range(
            self.source,
            &(call.callee.start..call.suffix.end),
        ) else {
            return Ok(repaired);
        };
        let mut widened = overrides.to_vec();
        blank_query_syntax(
            self.source,
            tola_typst_syntax::syntax::dropped(&statement),
            config,
            &mut widened,
        )?;
        Ok(compilations.inspect(session, config, source_revision, widened, cancellation)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position;
    use crate::query::meta::{call_around, meta_call, meta_key, meta_key_at};
    use crate::query::name_graph::bare_name;
    use crate::query::respond;
    use crate::query::semantic;
    use crate::query::tests::*;
    use crate::sources::SourceView;
    use lsp_types::CompletionTextEdit;
    use lsp_types::Position;
    use proptest::prelude::*;
    use serde_json::json;
    use tola_build::cancellation::BuildCancelled;
    use tola_build::diagnostic::Severity;
    use tola_typst::typst::syntax::LinkedNode;
    use tola_typst::typst::syntax::ast;

    #[test]
    fn stale_revisions_never_reuse_values() {
        let mut site = QuerySession::new();
        let items = site.completion(
            "#import \"@tola/document:0.0.0\": current-document\n#context current-document().|",
        );
        let valid = completion_labels(&items);
        assert!(valid.contains(&"output"), "{valid:?}");

        let invalid = site.reply(
            "textDocument/completion",
            "#import \"@tola/document:0.0.0\": current-document\n#context current-document().|\n#unresolved_after_cursor",
        );
        assert_eq!(invalid, json!([]));

        let checked = site
            .session
            .inspect(
                vec![(
                    site.config.build().content_dir.join("document.typ"),
                    Arc::from("#unresolved"),
                )],
                &BuildCancellation::new(),
            )
            .unwrap();
        assert!(!checked.compiled());
        assert!(
            checked
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
    }
    #[test]
    fn cancelled_queries_report_cancellation() {
        let mut site = QuerySession::new();

        let path = site.config.build().content_dir.join("document.typ");
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();
        let result = respond(
            &mut site.session,
            &mut site.compilations,
            &mut crate::analysis::DiskSources::default(),
            &mut site.bibliographies,
            &site.config,
            0,
            &[],
            &SourceView::default(),
            &[],
            None,
            &decode(
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": crate::uri::from_file_path(&path).unwrap().as_str() },
                    "position": { "line": 0, "character": 0 },
                }),
            ),
            false,
            CheckProgress::Failed,
            &canceller.token(),
            &crate::uri::ClientRoot::new(site.config.get_root()),
        );
        assert!(result.unwrap_err().is::<BuildCancelled>());
    }
    /// The text an editor holds open: ASCII markup, Han, emoji, combining marks, both line
    /// endings, and the calls and imports the query lanes classify.
    fn editor_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just("#import \"@tola/source:0.0.0\": tola-meta\n".to_owned()),
                Just("#tola-meta((title: \"Hello\", date: 2026))".to_owned()),
                Just("#tola-meta((标签: \"正文\", tag: \"😀\"))".to_owned()),
                Just("#tola-meta((title: \"Page\", summ))".to_owned()),
                Just("#let value = site.title\n".to_owned()),
                Just("#f(1, g(2))".to_owned()),
                Just("#context current-document().output".to_owned()),
                Just("plain ascii text".to_owned()),
                Just("正文与标点。".to_owned()),
                Just("😀🦊".to_owned()),
                Just("e\u{301}".to_owned()),
                Just("\r\n".to_owned()),
                Just("\n".to_owned()),
                Just(" ".to_owned()),
                Just("#(".to_owned()),
                Just("#let x =".to_owned()),
                Just("// 注释\n".to_owned()),
            ],
            0..8,
        )
        .prop_map(|fragments| fragments.concat())
    }

    /// The completion request the lanes classify a cursor with.
    fn completion_query(position: Position) -> SourceQuery {
        SourceQuery::Completion(lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: "file:///site/main.typ".parse().expect("a file URI"),
            },
            position,
        })
    }

    fn char_boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
        (0..=text.len()).filter(move |at| text.is_char_boundary(*at))
    }

    /// Whether the boundary at `at` lies between the two bytes of a `\r\n` break, which no
    /// protocol position addresses.
    fn inside_carriage_return_pair(text: &str, at: usize) -> bool {
        at > 0
            && at < text.len()
            && text.as_bytes()[at - 1] == b'\r'
            && text.as_bytes()[at] == b'\n'
    }

    /// Whether `character` can be part of a Typst identifier: the continue set, so a kebab-case
    /// name the author is still typing counts as a prefix too.
    fn is_name_character(character: char) -> bool {
        character.is_alphanumeric() || character == '_' || character == '-'
    }

    /// The identifier bytes the author typed before `cursor`, drawn from the document.
    fn identifier_prefix(text: &str, cursor: usize) -> std::ops::Range<usize> {
        let mut start = cursor;
        while start > 0 {
            let previous = text[..start].chars().next_back().expect("a character");
            if !is_name_character(previous) {
                break;
            }
            start -= previous.len_utf8();
        }
        start..cursor
    }

    /// The rest of a name an item could offer, so the label extends the typed prefix.
    fn identifier_tail() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                proptest::char::range('a', 'z'),
                proptest::char::range('A', 'Z'),
                proptest::char::range('\u{4E00}', '\u{9FFF}'),
                Just('_'),
            ],
            0..4,
        )
        .prop_map(String::from_iter)
    }

    /// The ranges of every call nested inside `node`.
    fn call_ranges(node: &LinkedNode<'_>) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let mut pending: Vec<LinkedNode<'_>> = node.children().collect();
        while let Some(child) = pending.pop() {
            if child.cast::<ast::FuncCall>().is_some() {
                ranges.push(child.range());
            }
            pending.extend(child.children());
        }
        ranges
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Direct(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/target/proptest/query.txt",
                )),
            )),
            cases: 128,
            ..ProptestConfig::default()
        })]

        /// A completion item's edit must rewrite exactly the prefix the author typed — no byte
        /// before it and none after it — for the unbounded space of documents and prefixes an
        /// editor holds; a violation corrupts the author's text around the completion.
        #[test]
        fn completion_edit_replaces_the_typed_prefix(
            text in editor_text(),
            tail in identifier_tail(),
        ) {
            let source = Source::detached(text.clone());
            for cursor in char_boundaries(&text) {
                if inside_carriage_return_pair(&text, cursor) {
                    continue;
                }
                let position = position::utf16_range(source.lines(), cursor..cursor)
                    .expect("a character boundary has a position")
                    .start;
                let selection = Selection::new(
                    &source,
                    cursor,
                    &completion_query(position),
                    CheckProgress::NotChecked,
                );
                let prefix = identifier_prefix(&text, cursor);
                let (range, typed) = selection
                    .completion_range(prefix.clone())
                    .expect("a typed prefix has a completion range");
                prop_assert_eq!(typed, &text[prefix.clone()]);
                let start = position::byte_offset(source.lines(), range.start)
                    .expect("the edit's start is a position");
                let end = position::byte_offset(source.lines(), range.end)
                    .expect("the edit's end is a position");
                prop_assert_eq!((start, end), (prefix.start, prefix.end));
                let label = format!("{typed}{tail}");
                let edit = selection
                    .completion_edit(prefix.clone(), label.clone())
                    .expect("a typed prefix has a completion edit");
                let item = semantic::completion(&label, &[], edit);
                prop_assert!(item.text_edit.is_some(), "a completion item has an edit");
                let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
                    panic!("a name completion arrives as one plain edit")
                };
                let applied = format!("{}{}{}", &text[..start], edit.new_text, &text[end..]);
                prop_assert_eq!(
                    applied,
                    format!("{}{}{}", &text[..prefix.start], label, &text[prefix.end..]),
                    "{:?} at {}",
                    text,
                    cursor
                );
            }
        }

        /// Every cursor walker must answer the construct the cursor sits in — the call it covers,
        /// the key it writes, the name the cursor reads — for the unbounded space of documents
        /// and cursors; a violation mis-targets the answer at that cursor.
        #[test]
        fn cursor_walkers_answer_at_the_cursor(text in editor_text()) {
            let source = Source::detached(text.clone());
            let document = source.text();
            let names = tola_typst_syntax::names::SourceNames::new(source.clone());
            for cursor in char_boundaries(document) {
                if let Some(node) = call_around(&source, cursor) {
                    prop_assert!(
                        node.range().start <= cursor && cursor <= node.range().end,
                        "call {:?} misses {}",
                        node.range(),
                        cursor
                    );
                    for inner in call_ranges(&node) {
                        if inner.start <= cursor && cursor <= inner.end {
                            prop_assert!(
                                inner.len() >= node.range().len(),
                                "call {:?} covers an inner call {:?} at {}",
                                node.range(),
                                inner,
                                cursor
                            );
                        }
                    }
                }
                if let Some(name) = bare_name(&source, cursor) {
                    let start = name.as_ptr() as usize - document.as_ptr() as usize;
                    let end = start + name.len();
                    prop_assert!(end <= document.len(), "name {:?} at {}", name, cursor);
                    // The walker's own reader answers the name only where the cursor reads a
                    // value, and there the name must be the one the cursor stands in.
                    prop_assert!(
                        !names.reads_value_at(cursor) || (start <= cursor && cursor <= end),
                        "name {:?} misses the value cursor at {}",
                        name,
                        cursor
                    );
                }
                if let Some(call) = meta_call(&source, cursor) {
                    if let Some(key) = meta_key(&source, cursor, &call) {
                        prop_assert!(
                            key.typed.start <= cursor && cursor <= key.typed.end,
                            "typed key {:?} misses {}",
                            key.typed,
                            cursor
                        );
                        prop_assert!(
                            call.dict.start <= key.typed.start && key.typed.end <= call.dict.end,
                            "typed key {:?} escapes dict {:?} at {}",
                            key.typed,
                            call.dict,
                            cursor
                        );
                    }
                    if let Some(key) = meta_key_at(&source, cursor, &call) {
                        prop_assert!(
                            key.start <= cursor && cursor <= key.end,
                            "key {:?} misses {}",
                            key,
                            cursor
                        );
                    }
                }
            }
        }

    }
}
