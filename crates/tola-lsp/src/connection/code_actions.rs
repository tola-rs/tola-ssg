//! Code action assembly: buffer-local fixes and deferred site narrowing/selection waits.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use lsp_server::RequestId;
use lsp_types::{CodeActionKind, CodeActionOrCommand, CodeActionParams, NumberOrString, TextEdit};
use tola_diagnostic::Diagnostic;
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind};
use tola_typst_syntax::names::SourceNames;

use crate::code_actions::{
    admits, diagnostic_actions, narrowing_closure, only_names, selection_actions,
};
use crate::compiler::{QueryRequest, SelectionRead, SourceJob};
use crate::diagnostic::UnreadImports;
use crate::links::Link;
use crate::protocol::{self, SourceQuery, SourceReply};
use crate::sources::SourceView;

use super::replies::{PANICKED_ERROR, error_response};
use super::{Connection, PendingSelection, Phase, SelectionBuild};

#[derive(Clone, Copy)]
struct RequestKinds {
    quickfix: bool,
    organize: bool,
    refactor: bool,
    fix_all: bool,
}

impl RequestKinds {
    fn of(req: &CodeActionParams) -> Self {
        let only = req.context.only.as_deref();
        Self {
            quickfix: admits(only, &CodeActionKind::QUICKFIX),
            organize: admits(only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            refactor: admits(only, &CodeActionKind::REFACTOR_REWRITE),
            fix_all: only_names(only, &CodeActionKind::SOURCE_FIX_ALL),
        }
    }

    fn asks_for_reports(self) -> bool {
        self.quickfix || self.organize || self.fix_all
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn code_actions(
        &mut self,
        id: RequestId,
        mut params: CodeActionParams,
    ) -> Result<()> {
        let root = self.source_root();
        let kinds = RequestKinds::of(&params);
        let uri = params.text_document.uri.clone();
        let src = self.source(&uri);
        let view = self.open.view();
        let mut names = kinds
            .asks_for_reports()
            .then(|| view.get(&uri).map(|parsed| parsed.names()))
            .flatten();
        // Clients without diagnostic data support cannot echo causes back, so substitute stored diagnostics.
        let echoes_causes = matches!(
            &self.phase,
            Phase::Ready(ws) if ws.features.reads_diagnostic_data()
        );
        let published = (kinds.fix_all || !echoes_causes)
            .then(|| self.published_diagnostics(&uri))
            .flatten();
        if !self.reconcile_reports(
            &uri,
            &root,
            src.as_ref(),
            kinds,
            echoes_causes,
            published.as_deref(),
            &mut params,
        )? {
            return self.await_selection(id, params, root);
        }
        let mut actions = match &src {
            Some(src) => {
                let mut actions = if kinds.quickfix {
                    text_fixes(&uri, src, &mut names, &view, &root, params.range)
                } else {
                    Vec::new()
                };
                actions.extend(diagnostic_actions(
                    &uri,
                    src,
                    names.as_deref(),
                    &params,
                    self.host_sections,
                ));
                if kinds.refactor {
                    actions.extend(selection_actions(&uri, src, params.range));
                }
                actions
            }
            None => self.report_fixes(&uri, &root, echoes_causes, published.as_deref(), &params),
        };
        filter_by_kind(&mut actions, params.context.only.as_deref());
        // Narrowing needs the checked site world; hold local actions until the compiler lane answers.
        if kinds.refactor
            && let Some(src) = &src
            && let Ok(cursor) = crate::position::byte_offset(src.lines(), params.range.start)
            && narrowing_closure(src, cursor).is_some()
        {
            return self.await_narrowing(id, params, view, actions);
        }
        self.source_reply(
            id,
            SourceReply::CodeActions((!actions.is_empty()).then_some(actions)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reconcile_reports(
        &mut self,
        uri: &lsp_types::Uri,
        root: &Path,
        src: Option<&Source>,
        kinds: RequestKinds,
        echoes_causes: bool,
        published: Option<&[lsp_types::Diagnostic]>,
        params: &mut CodeActionParams,
    ) -> Result<bool> {
        if !echoes_causes && kinds.asks_for_reports() {
            params.context.diagnostics = published.unwrap_or_default().to_vec();
        }
        let echoed_unread = params.context.diagnostics.iter().any(is_unread_import);
        let asks_unread =
            kinds.asks_for_reports() && (kinds.organize || !echoes_causes || echoed_unread);
        if !asks_unread || crate::uri::to_site_path(uri.as_str()).is_err() {
            return Ok(true);
        }
        let Some(src) = src else {
            return Ok(true);
        };
        let unread = match self.workspace().diagnostics.unused_imports(
            self.revision,
            root,
            &self.open,
            uri,
            src,
        )? {
            UnreadImports::Answered(unread) => unread,
            UnreadImports::NeedsSelection => return Ok(false),
        };
        if echoes_causes {
            params.context.diagnostics.retain(|diag| {
                !is_unread_import(diag)
                    || (!kinds.organize
                        && unread
                            .iter()
                            .any(|auth| auth.range == diag.range && auth.data == diag.data))
            });
            if kinds.organize {
                params.context.diagnostics.extend(unread);
            } else if kinds.fix_all {
                extend_with_unheld(&mut params.context.diagnostics, &unread);
            }
        } else {
            params.context.diagnostics.retain(|diag| {
                !unread
                    .iter()
                    .any(|fresh| fresh.range == diag.range && fresh.code == diag.code)
            });
            let range = params.range;
            params
                .context
                .diagnostics
                .extend(unread.into_iter().filter(|diag| {
                    kinds.fix_all
                        || (diag.range.start <= range.end && diag.range.end >= range.start)
                }));
        }
        if kinds.fix_all
            && echoes_causes
            && let Some(reports) = published
        {
            extend_with_unheld(&mut params.context.diagnostics, reports);
        }
        Ok(true)
    }

    fn await_narrowing(
        &mut self,
        id: RequestId,
        params: CodeActionParams,
        view: SourceView,
        actions: Vec<CodeActionOrCommand>,
    ) -> Result<()> {
        let routes_as_hints = self.workspace().routes_as_hints;
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.requests
            .get_mut(&id)
            .expect("admitted code action request")
            .narrowing = Some(actions);
        self.queries.push_back(SourceJob::Query(QueryRequest {
            id,
            serial: admission.serial,
            sources: self.source_inputs(self.workspace().root.clone(), admission.cancellation),
            view,
            routes_as_hints,
            check_progress: self.check_progress(),
            query: SourceQuery::CodeActions(params),
        }));
        Ok(())
    }

    fn report_fixes(
        &self,
        uri: &lsp_types::Uri,
        root: &Path,
        echoes_causes: bool,
        published: Option<&[lsp_types::Diagnostic]>,
        params: &CodeActionParams,
    ) -> Vec<CodeActionOrCommand> {
        let mut req = params.clone();
        if !echoes_causes {
            req.context.diagnostics = published.unwrap_or_default().to_vec();
        }
        if !req
            .context
            .diagnostics
            .iter()
            .any(|diag| diag.data.is_some())
        {
            return Vec::new();
        }
        let Ok(path) = crate::uri::to_site_path(uri.as_str()) else {
            return Vec::new();
        };
        if !(path.starts_with(root) || path == self.configuration_path(root)) {
            return Vec::new();
        }
        let Some(text) = self.document_text(&path) else {
            return Vec::new();
        };
        diagnostic_actions(
            uri,
            &Source::detached(text.as_ref()),
            None,
            &req,
            self.host_sections,
        )
    }

    fn await_selection(
        &mut self,
        id: RequestId,
        params: lsp_types::CodeActionParams,
        root: PathBuf,
    ) -> Result<()> {
        let rev = self.revision;
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.requests
            .get_mut(&id)
            .expect("admitted code action request")
            .waiting = Some(PendingSelection {
            revision: rev,
            params,
        });
        if self.selection_build.is_none() {
            let canceller = tola_build::cancellation::BuildCanceller::new();
            let cancel = canceller.token();
            self.selection_build = Some(SelectionBuild {
                revision: rev,
                serial: admission.serial,
                canceller,
            });
            self.queries.push_back(SourceJob::Selection(SelectionRead {
                revision: rev,
                serial: admission.serial,
                root,
                boundary: self.source_boundary(),
                view: self.open.view(),
                cancellation: cancel,
            }));
        }
        Ok(())
    }

    pub(super) fn waiting_code_actions(&self, rev: u64) -> Vec<RequestId> {
        self.requests
            .iter()
            .filter_map(|(id, req)| {
                let waiting = req.waiting.as_ref()?;
                (waiting.revision == rev).then(|| id.clone())
            })
            .collect()
    }

    pub(super) fn complete_waiting_code_actions(&mut self, rev: u64) -> Result<()> {
        for id in self.waiting_code_actions(rev) {
            let req = self.requests.remove(&id).expect("a waiting code action");
            let pending = req
                .waiting
                .expect("a waiting code action holds its own request");
            self.code_actions(id, pending.params)?;
        }
        Ok(())
    }

    pub(super) fn fail_waiting_code_actions(&mut self, rev: u64) -> Result<()> {
        for id in self.waiting_code_actions(rev) {
            self.requests.remove(&id);
            self.reply(error_response(id, PANICKED_ERROR))?;
        }
        Ok(())
    }
}

fn filter_by_kind(actions: &mut Vec<CodeActionOrCommand>, only: Option<&[CodeActionKind]>) {
    actions.retain(|action| match action {
        CodeActionOrCommand::CodeAction(action) => {
            action.kind.as_ref().is_none_or(|kind| admits(only, kind))
        }
        CodeActionOrCommand::Command(_) => only.is_none_or(<[_]>::is_empty),
    });
}

fn is_unread_import(diag: &lsp_types::Diagnostic) -> bool {
    diag.code.as_ref().is_some_and(|code| match code {
        NumberOrString::String(code) => {
            code.as_str() == tola_diagnostic::codes::check::UNUSED_IMPORT.as_str()
        }
        NumberOrString::Number(_) => false,
    }) || diag
        .data
        .as_ref()
        .and_then(|cause| cause.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some("unread-import")
}

fn text_fixes(
    uri: &lsp_types::Uri,
    src: &Source,
    names: &mut Option<Arc<SourceNames>>,
    view: &SourceView,
    root: &Path,
    range: lsp_types::Range,
) -> Vec<CodeActionOrCommand> {
    let (Ok(start), Ok(end)) = (
        crate::position::byte_offset(src.lines(), range.start),
        crate::position::byte_offset(src.lines(), range.end),
    ) else {
        return Vec::new();
    };
    if start > end {
        return Vec::new();
    }
    let names = names.get_or_insert_with(|| Arc::new(SourceNames::new(src.clone())));
    let is_named = !view.is_unnamed(src.id());
    let links = if is_named {
        crate::links::declared(src)
    } else {
        Vec::new()
    };
    let mut fixes = Vec::new();
    let mut seen = BTreeSet::new();
    let mut cursors = Vec::new();
    action_cursors(LinkedNode::new(src.root()), start..end, &mut cursors);
    for cursor in cursors {
        for (title, edit) in cursor_fixes(src, cursor, names, &links, is_named) {
            let key = (edit.range.start, edit.range.end, edit.new_text.clone());
            if seen.insert(key) {
                fixes.push(protocol::quick_fix(uri, title, edit));
            }
        }
    }
    fixes.extend(create_missing_targets(root, range, &links));
    fixes
}

fn cursor_fixes(
    src: &Source,
    cursor: usize,
    names: &SourceNames,
    links: &[Link],
    is_named: bool,
) -> Vec<(String, TextEdit)> {
    let mut fixes = Vec::new();
    if let Some(edit) = crate::query::package::replacement(src, cursor) {
        fixes.push((
            format!("replace the package with `{}`", edit.new_text),
            edit,
        ));
    }
    fixes.extend(crate::query::package::missing_import(src, names, cursor));
    if is_named {
        let link = crate::links::holding(links, cursor);
        fixes.extend(crate::query::paths::rewrites(src, cursor, link, names));
    }
    fixes
}

fn create_missing_targets(
    root: &Path,
    range: lsp_types::Range,
    links: &[Link],
) -> Vec<CodeActionOrCommand> {
    let mut offered = BTreeSet::new();
    let mut fixes = Vec::new();
    for link in links {
        if link.range.start > range.end || link.range.end < range.start {
            continue;
        }
        if let Some(path) = link.site_path(root)
            && offered.insert(path.clone())
            && !path.exists()
            && let Some(create) = protocol::create_file(&path)
        {
            fixes.push(create);
        }
    }
    fixes
}

fn extend_with_unheld<'a>(
    held: &mut Vec<lsp_types::Diagnostic>,
    reports: impl IntoIterator<Item = &'a lsp_types::Diagnostic>,
) {
    for report in reports {
        if !held
            .iter()
            .any(|echoed| echoed.range == report.range && echoed.code == report.code)
        {
            held.push(report.clone());
        }
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
    use crate::compiler::{SourceCompilation, SourceJob};
    use crate::connection::tests::{messages, open_document, ready_at, ready_workspace, response};
    use crate::server::tests::load_configuration;
    use lsp_server::Request;

    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::Path;
    use std::time::Instant;

    fn declare_quickfix_support(conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
        ready_workspace(conn).features = ClientFeatures::new(
            &serde_json::from_value(json!({
                "textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}}}}
            }))
            .unwrap(),
        );
    }

    fn answer_queued_check(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        compile: impl FnOnce(SourceJob) -> SourceCompilation,
    ) {
        let checked = compile(conn.next_job(Instant::now()).expect("a queued check"));
        conn.completed(checked).unwrap();
        messages(conn);
    }

    #[test]
    fn quickfix_without_echoed_causes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(root.join("content/helpers.typ"), "#let orphan = 42\n").unwrap();
        let uri = crate::uri::from_file_path(&root.join("content/page.typ")).unwrap();
        let text = "#import \"helpers.typ\": orphan\nBody.\n";
        std::fs::write(root.join("content/page.typ"), text).unwrap();
        let mut conn = ready_at(Vec::new(), root.to_path_buf());
        declare_quickfix_support(&mut conn);
        open_document(&mut conn, &uri, 1, text);
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path,
             _: &[(PathBuf, Arc<str>)],
             _: &tola_build::cancellation::BuildCancellation| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        answer_queued_check(&mut conn, |job| compiler.compile(job));
        let src = conn.source(&uri).unwrap();
        let byte = text.find("orphan").unwrap();
        let cursor = crate::position::utf16_range(src.lines(), byte..byte)
            .unwrap()
            .start;
        conn.request(Request::new(
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
            serde_json::from_value(response(&mut conn).response_result.unwrap()).unwrap();
        let actions = actions.unwrap_or_default();
        assert_eq!(actions.len(), 1, "{actions:?}");
        let lsp_types::CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("a quick fix")
        };
        assert_eq!(action.kind, Some(lsp_types::CodeActionKind::QUICKFIX));
        assert!(action.edit.is_some(), "{action:?}");
    }
}
