//! Code action assembly: the fixes this connection derives from the text the author is looking at,
//! and the wait for the narrowing actions the checked world adds.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use lsp_server::RequestId;
use lsp_types::NumberOrString;
use tola_build::diagnostic::Diagnostic;
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind};

use crate::compiler::{QueryRequest, SelectionRead, SourceJob, checked_root};
use crate::diagnostic::UnreadImports;
use crate::protocol::{self, SourceQuery, SourceReply};

use super::replies::failed_response;
use super::{Connection, PendingSelection, Phase};

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn code_actions(
        &mut self,
        id: RequestId,
        mut params: lsp_types::CodeActionParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        // A correction reads the same site a check's own report does, under the one rule that names
        // it: the resolved configuration's root, or the workspace's until one resolves.
        let root = tola_build::filesystem::normalize_existing_prefix(checked_root(
            self.configuration.as_ref(),
            &workspace.root,
        ));
        let only = params.context.only.as_deref();
        let quickfix = crate::code_actions::admits(only, &lsp_types::CodeActionKind::QUICKFIX);
        let organize =
            crate::code_actions::admits(only, &lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS);
        let refactor =
            crate::code_actions::admits(only, &lsp_types::CodeActionKind::REFACTOR_REWRITE);
        let fix_all =
            crate::code_actions::only_names(only, &lsp_types::CodeActionKind::SOURCE_FIX_ALL);
        let uri = &params.text_document.uri;
        let source = self.source(uri);
        let view = self.open.view();
        let mut source_names = if quickfix || organize || fix_all {
            view.get(uri).map(|snapshot| snapshot.names())
        } else {
            None
        };
        let is_unread_import = |diagnostic: &lsp_types::Diagnostic| {
            diagnostic
                .data
                .as_ref()
                .and_then(|cause| cause.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("unread-import")
        };
        // A client that declared no diagnostic data echoes the reports without the causes a fix is
        // built from, so the reports the server itself holds answer in their place; one that keeps
        // the causes answers from the echo, which is the version it asked about.
        let echoes_causes = matches!(
            &self.phase,
            Phase::Ready(workspace) if workspace.features.reads_diagnostic_data()
        );
        let unread_import_code = tola_build::codes::check::UNUSED_IMPORT.to_string();
        let unread_import = |diagnostic: &lsp_types::Diagnostic| {
            is_unread_import(diagnostic)
                || diagnostic.code.as_ref().is_some_and(|code| {
                    matches!(code, NumberOrString::String(code) if code == &unread_import_code)
                })
        };
        // The reports the last completed check published for this document: they answer a fix
        // request when the client cannot echo their causes, and they complete a fix-all the client
        // scoped to fewer reports than the document's own check found.
        let published = (fix_all || !echoes_causes)
            .then(|| self.published_diagnostics(uri))
            .flatten();
        if !echoes_causes && (quickfix || organize || fix_all) {
            params.context.diagnostics = published.clone().unwrap_or_default();
        }
        // The document's own analysis answers its unread imports, whether or not the client echoed
        // them: a stale echo is dropped and a missing one is supplied.
        let asks_unread_imports = organize
            || (quickfix || fix_all)
                && (!echoes_causes || params.context.diagnostics.iter().any(unread_import));
        if asks_unread_imports
            && crate::uri::to_site_path(uri.as_str()).is_ok()
            && let Some(source) = &source
        {
            let workspace = self.workspace();
            let current = match workspace.diagnostics.unused_imports(
                self.revision,
                &root,
                &self.open,
                uri,
                source,
            )? {
                UnreadImports::Answered(current) => current,
                UnreadImports::NeedsSelection => {
                    // A file-scope statement is another source's to select, and no completed check
                    // of this revision returned the index that judges it: the request waits for
                    // the compiler lane to build that very index rather than reading a stale one.
                    return self.await_selection(id, params.clone(), root);
                }
            };
            if echoes_causes {
                params.context.diagnostics.retain(|diagnostic| {
                    !unread_import(diagnostic)
                        || (!organize
                            && current.iter().any(|authorized| {
                                authorized.range == diagnostic.range
                                    && authorized.data == diagnostic.data
                            }))
                });
                if organize {
                    params.context.diagnostics.extend(current);
                } else if fix_all {
                    // A fix-all answers for every unread import the document's own analysis
                    // justifies, not only the ones the client echoed.
                    for report in current {
                        if !params.context.diagnostics.iter().any(|echoed| {
                            echoed.range == report.range && echoed.code == report.code
                        }) {
                            params.context.diagnostics.push(report);
                        }
                    }
                }
            } else {
                // The causes the server holds answer the file; the document's own analysis answers
                // its unread imports from the buffer the author is looking at. A plain quick-fix
                // asks about the range it stands in, a fix-all about the whole file.
                params.context.diagnostics.retain(|report| {
                    !current
                        .iter()
                        .any(|fresh| fresh.range == report.range && fresh.code == report.code)
                });
                let range = params.range;
                let asked = |report: &lsp_types::Diagnostic| {
                    fix_all || (report.range.start <= range.end && report.range.end >= range.start)
                };
                params
                    .context
                    .diagnostics
                    .extend(current.into_iter().filter(asked));
            }
        }
        if fix_all
            && echoes_causes
            && let Some(reports) = &published
        {
            for report in reports {
                if !params
                    .context
                    .diagnostics
                    .iter()
                    .any(|echoed| echoed.range == report.range && echoed.code == report.code)
                {
                    params.context.diagnostics.push(report.clone());
                }
            }
        }
        let mut actions = Vec::new();
        if quickfix
            && let Some(source) = &source
            && let Ok(start) = crate::position::byte_offset(source.lines(), params.range.start)
            && let Ok(end) = crate::position::byte_offset(source.lines(), params.range.end)
            && start <= end
        {
            let names = source_names.get_or_insert_with(|| {
                Arc::new(tola_typst_syntax::names::SourceNames::new(source.clone()))
            });
            let named = !view.is_unnamed(source.id());
            let links = if named {
                crate::links::paths(source, &root).unwrap_or_default()
            } else {
                Vec::new()
            };
            let mut cursors = Vec::new();
            action_cursors(LinkedNode::new(source.root()), start..end, &mut cursors);
            let mut edits = std::collections::BTreeSet::new();
            for cursor in cursors {
                let mut push = |title: String, edit: lsp_types::TextEdit| {
                    let key = (edit.range.start, edit.range.end, edit.new_text.clone());
                    if edits.insert(key) {
                        actions.push(protocol::quick_fix(uri, title, edit));
                    }
                };
                if let Some(edit) = crate::query::package::replacement(source, cursor) {
                    push(
                        format!("replace the package with `{}`", edit.new_text),
                        edit,
                    );
                }
                for (title, edit) in crate::query::package::missing_import(source, names, cursor) {
                    push(title, edit);
                }
                if named {
                    // `cursor` names a leaf of this source, so the position always exists; a source
                    // that disagreed answers no fix rather than ending the connection.
                    let Some(at) = crate::position::utf16_range(source.lines(), cursor..cursor)
                        .map(|range| range.start)
                    else {
                        continue;
                    };
                    let written = links
                        .iter()
                        .find(|link| link.range.start <= at && at <= link.range.end)
                        .map(|link| link.range);
                    for (title, edit) in crate::query::paths::rewrites(source, cursor, written) {
                        push(title, edit);
                    }
                }
            }
            let mut created = std::collections::BTreeSet::new();
            for link in links {
                if link.range.start > params.range.end || link.range.end < params.range.start {
                    continue;
                }
                if let Some(target) = link.target
                    && let Ok(path) = crate::uri::to_site_path(target.as_str())
                    && path.starts_with(&root)
                    && created.insert(path.clone())
                    && !path.exists()
                    && let Some(create) = protocol::create_file(&path)
                {
                    actions.push(create);
                }
            }
        }
        match &source {
            Some(source) => {
                actions.extend(crate::code_actions::diagnostic_actions(
                    uri,
                    source,
                    source_names.as_deref(),
                    &params,
                    self.host_sections,
                ));
                if refactor {
                    actions.extend(crate::code_actions::selection_actions(
                        uri,
                        source,
                        params.range,
                    ));
                }
            }
            None => {
                // Only a diagnostic that has a cause justifies a fix; a client that cannot echo
                // causes is answered from the reports the server itself holds for the document.
                let mut request = params.clone();
                if !echoes_causes {
                    request.context.diagnostics = published.clone().unwrap_or_default();
                }
                let justified = request
                    .context
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.data.is_some());
                if justified
                    && let Ok(path) = crate::uri::to_site_path(uri.as_str())
                    && (path.starts_with(&root) || path == self.configuration_path(&root))
                    && let Some(text) = self.document_text(&path)
                {
                    actions.extend(crate::code_actions::diagnostic_actions(
                        uri,
                        &Source::detached(text.as_ref()),
                        None,
                        &request,
                        self.host_sections,
                    ));
                }
            }
        }
        // One filter for the whole reply: an editor that names the kinds it wants is answered with
        // those kinds alone, so no lane's fix rides along with another lane's action. A kind is
        // hierarchical, so a request for `refactor` admits `refactor.rewrite`.
        let only = params.context.only.as_deref();
        actions.retain(|action| match action {
            lsp_types::CodeActionOrCommand::CodeAction(action) => action
                .kind
                .as_ref()
                .is_none_or(|kind| crate::code_actions::admits(only, kind)),
            // A command has no kind, so only a request without a filter receives one.
            lsp_types::CodeActionOrCommand::Command(_) => only.is_none_or(<[_]>::is_empty),
        });
        // The narrowing refactor reads the checked world, so a request whose cursor sits in a
        // collection closure is answered by the compiler lane: the fixes computed here wait for
        // the narrowing actions the world adds.
        if refactor
            && let Some(source) = &source
            && let Ok(cursor) = crate::position::byte_offset(source.lines(), params.range.start)
            && crate::code_actions::narrowing_closure(source, cursor).is_some()
        {
            let workspace = self.workspace();
            let package_sources = workspace.package_sources.clone();
            let routes_as_hints = workspace.routes_as_hints;
            let Some((serial, cancellation)) = self.admit(&id)? else {
                return Ok(());
            };
            self.requests
                .get_mut(&id)
                .expect("admitted code action request")
                .narrowing = Some(actions);
            self.queries.push_back(SourceJob::Query(QueryRequest {
                id,
                serial,
                sources: self.source_inputs(root, cancellation),
                view,
                package_sources,
                routes_as_hints,
                check_progress: self.check_progress(),
                query: SourceQuery::CodeActions(params),
            }));
            return Ok(());
        }
        self.source_reply(
            id,
            SourceReply::CodeActions((!actions.is_empty()).then_some(actions)),
        )
    }

    /// Hold one code action until the compiler lane has built its revision's selection index.
    ///
    /// The revision has no completed check behind it — the author is still typing, or the site
    /// checks on save — so the index a file-scope unread import is judged against does not exist
    /// yet. The lane builds exactly the index this request reads, and the request is answered from
    /// it unchanged the moment it arrives; this connection never walks the site itself.
    fn await_selection(
        &mut self,
        id: RequestId,
        params: lsp_types::CodeActionParams,
        root: PathBuf,
    ) -> Result<()> {
        let revision = self.revision;
        if self.admit(&id)?.is_none() {
            return Ok(());
        }
        self.requests
            .get_mut(&id)
            .expect("admitted code action request")
            .waiting = Some(PendingSelection { revision, params });
        if self.selection_build != Some(revision) {
            // One build answers every correction this revision waits for, and a source change ends
            // it with the revision it was started for.
            self.selection_build = Some(revision);
            self.queries.push_back(SourceJob::Selection(SelectionRead {
                revision,
                root,
                view: self.open.view(),
                cancellation: self.checking.token(),
            }));
        }
        Ok(())
    }

    fn waiting_code_actions(&self, revision: u64) -> Vec<RequestId> {
        self.requests
            .iter()
            .filter_map(|(id, request)| {
                let waiting = request.waiting.as_ref()?;
                (waiting.revision == revision).then(|| id.clone())
            })
            .collect()
    }

    /// Complete every code action held for `revision`, now that its selection index exists.
    ///
    /// A request the client cancelled, or a source change superseded, is no longer this
    /// connection's to answer: it left `requests` with its own reply.
    pub(super) fn complete_waiting_code_actions(&mut self, revision: u64) -> Result<()> {
        let waiting = self.waiting_code_actions(revision);
        for id in waiting {
            let request = self.requests.remove(&id).expect("a waiting code action");
            let waiting = request
                .waiting
                .expect("a waiting code action holds its own request");
            self.code_actions(id, waiting.params)?;
        }
        Ok(())
    }

    /// Answer every code action held for `revision` with the internal failure its index build
    /// ended in.
    ///
    /// A selection build has one failure the lane reports itself — a panicked job, logged with the
    /// panic's own message — so each request is answered in the protocol's shape for an unexpected
    /// failure rather than left waiting on an index that will never arrive.
    pub(super) fn fail_waiting_code_actions(&mut self, revision: u64) -> Result<()> {
        let waiting = self.waiting_code_actions(revision);
        for id in waiting {
            self.requests.remove(&id);
            self.reply(failed_response(
                id,
                anyhow::Error::new(crate::server::PanickedJob),
            ))?;
        }
        Ok(())
    }
}

fn action_cursors(node: LinkedNode<'_>, range: std::ops::Range<usize>, cursors: &mut Vec<usize>) {
    let span = node.range();
    if span.start > range.end || span.end < range.start {
        return;
    }
    match node.kind() {
        SyntaxKind::Str => cursors.push((span.start + 1).min(span.end)),
        SyntaxKind::Ident | SyntaxKind::MathIdent => cursors.push(span.end),
        _ => {
            for child in node.children() {
                action_cursors(child, range.clone(), cursors);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ClientFeatures;
    use crate::connection::tests::{messages, open_document, ready_at, ready_workspace, response};
    use crate::protocol::CheckMode;
    use crate::server::tests::load_configuration;
    use lsp_server::Request;
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::Path;
    use std::time::Instant;
    use tola_typst::typst::syntax::Source;

    /// Every cursor the code-action lane derives sits at a position the source has, including one
    /// at its final byte: the lane reads a range at each cursor, so a source that disagreed answers
    /// no fix rather than ending the connection.
    #[test]
    fn code_action_cursors_stay_inside_their_source() {
        for text in ["#let value = other", "#let s = \"text\"", "#let n = 1 + y"] {
            let source = Source::detached(text);
            let mut cursors = Vec::new();
            action_cursors(LinkedNode::new(source.root()), 0..text.len(), &mut cursors);
            assert!(!cursors.is_empty(), "{text} answers a cursor");
            for cursor in cursors {
                assert!(
                    cursor <= text.len(),
                    "{text} keeps {cursor} inside the text"
                );
                assert!(
                    crate::position::utf16_range(source.lines(), cursor..cursor).is_some(),
                    "{text} addresses the cursor at {cursor}"
                );
            }
        }
        // A name ending the file puts its cursor exactly at the end, which is the position the
        // lane's own read must survive.
        let source = Source::detached("#let value = other");
        let mut cursors = Vec::new();
        action_cursors(
            LinkedNode::new(source.root()),
            0..source.text().len(),
            &mut cursors,
        );
        assert!(
            cursors.contains(&source.text().len()),
            "the name ending the file answers the end of it: {cursors:?}"
        );
    }

    #[test]
    fn quickfix_preserves_new_interface_selection() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(root.join("content/helpers.typ"), "#let orphan = 42\n").unwrap();
        std::fs::write(root.join("content/consumer.typ"), "#let placeholder = 0\n").unwrap();
        let target = "#import \"helpers.typ\": orphan\n#let kept = 1\n";
        let uri = crate::uri::from_file_path(&root.join("content/page.typ")).unwrap();
        let mut connection = ready_at(Vec::new(), root.to_path_buf());
        let workspace = ready_workspace(&mut connection);
        workspace.check_mode = CheckMode::OnSave;
        workspace.features = ClientFeatures::new(&serde_json::from_value(json!({
            "textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}}}}
        })).unwrap());
        open_document(&mut connection, &uri, 1, target);
        // A correction reads the index the revision's own completed check returned.
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        messages(&mut connection);
        let source = connection.source(&uri).unwrap();
        let Phase::Ready(workspace) = &mut connection.phase else {
            unreachable!()
        };
        let UnreadImports::Answered(diagnostics) = workspace
            .diagnostics
            .unused_imports(connection.revision, root, &connection.open, &uri, &source)
            .unwrap()
        else {
            panic!("the revision's check returned its index");
        };
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let range = diagnostics[0].range;
        open_document(
            &mut connection,
            &crate::uri::from_file_path(&root.join("content/consumer.typ")).unwrap(),
            2,
            "#import \"page.typ\": orphan\n#let selected = orphan\n",
        );
        messages(&mut connection);
        // The consumer selects `orphan` from the page, so the revision's check returns the index
        // that licenses the import and leaves the correction nothing to remove.
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        messages(&mut connection);
        connection
            .request(Request::new(
                8.into(),
                request::CodeActionRequest::METHOD.into(),
                json!({
                    "textDocument":{"uri":uri.as_str()},"range":range,
                    "context":{"diagnostics":diagnostics,"only":["quickfix"]}
                }),
            ))
            .unwrap();
        let actions: Option<Vec<lsp_types::CodeActionOrCommand>> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        for action in actions.unwrap_or_default() {
            let lsp_types::CodeActionOrCommand::CodeAction(action) = action else {
                continue;
            };
            if let Some(changes) = action.edit.and_then(|edit| edit.changes) {
                assert!(
                    changes
                        .values()
                        .flatten()
                        .all(|edit| !edit.new_text.is_empty())
                );
            }
        }
    }

    /// A client that declared no diagnostic data cannot echo a cause, so the document's own
    /// analysis answers its unread imports.
    #[test]
    fn quickfix_answers_without_echoed_causes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/helpers.typ"), "#let orphan = 42\n").unwrap();
        let uri = crate::uri::from_file_path(&root.join("content/page.typ")).unwrap();
        let text = "#import \"helpers.typ\": orphan\nBody.\n";
        std::fs::write(root.join("content/page.typ"), text).unwrap();
        let mut connection = ready_at(Vec::new(), root.to_path_buf());
        let workspace = ready_workspace(&mut connection);
        // A client that keeps no diagnostic data: its request has no cause to answer from.
        workspace.features = ClientFeatures::new(
            &serde_json::from_value(json!({
                "textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}}}}
            }))
            .unwrap(),
        );
        open_document(&mut connection, &uri, 1, text);
        // A correction reads the index the revision's own completed check returned.
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        messages(&mut connection);
        let source = connection.source(&uri).unwrap();
        let byte = text.find("orphan").unwrap();
        let cursor = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        connection
            .request(Request::new(
                8.into(),
                request::CodeActionRequest::METHOD.into(),
                json!({
                    "textDocument":{"uri":uri.as_str()},
                    "range":{"start":cursor,"end":cursor},
                    "context":{"diagnostics":[],"only":["quickfix"]}
                }),
            ))
            .unwrap();
        let actions: Option<Vec<lsp_types::CodeActionOrCommand>> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        let actions = actions.unwrap_or_default();
        assert_eq!(actions.len(), 1, "{actions:?}");
        let lsp_types::CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("a quick fix")
        };
        assert_eq!(action.kind, Some(lsp_types::CodeActionKind::QUICKFIX));
        assert!(action.edit.is_some(), "{action:?}");
    }
}
