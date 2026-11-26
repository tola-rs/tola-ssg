//! Source request dispatch to the analysis or compiler lane.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use lsp_server::RequestId;
use lsp_types::{
    CallHierarchyIncomingCallsParams, CodeLensParams, RenameFilesParams, Uri, WorkspaceSymbolParams,
};
use tola_diagnostic::Diagnostic;

use crate::compiler::{
    AnalysisRequest, AnalysisTask, IncomingCallsRead, LensesRead, PackageSourceRead, QueryRequest,
    RenameRead, RouteIndexRead, RouteRead, SourceInputs, SourceJob, SymbolsRead,
};
use crate::protocol::{self, PackageSourceParams, RouteParams, SourceQuery, SourceReply};

use super::local_requests::is_untitled;
use super::replies::failed_response;
use super::{Connection, Phase};

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    fn queue(
        &mut self,
        id: RequestId,
        job: impl FnOnce(RequestId, u64, SourceInputs) -> SourceJob,
    ) -> Result<()> {
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        let sources = self.source_inputs(self.workspace().root.clone(), admission.cancellation);
        self.queries.push_back(job(id, admission.serial, sources));
        Ok(())
    }

    pub(super) fn route(&mut self, id: RequestId, params: RouteParams) -> Result<()> {
        if is_untitled(&params.uri) {
            return self.source_reply(id, SourceReply::Routes(protocol::RouteReply::default()));
        }
        self.queue(id, |id, serial, sources| {
            SourceJob::Route(RouteRead {
                id,
                serial,
                sources,
                uri: params.uri,
            })
        })
    }

    pub(super) fn route_index(&mut self, id: RequestId) -> Result<()> {
        self.queue(id, |id, serial, sources| {
            SourceJob::RouteIndex(RouteIndexRead {
                id,
                serial,
                sources,
            })
        })
    }

    pub(super) fn code_lens(&mut self, id: RequestId, params: CodeLensParams) -> Result<()> {
        if is_untitled(&params.text_document.uri) {
            return self.source_reply(id, SourceReply::CodeLenses(None));
        }
        self.queue(id, |id, serial, sources| {
            SourceJob::Lenses(LensesRead {
                id,
                serial,
                sources,
                uri: params.text_document.uri,
            })
        })
    }

    pub(super) fn workspace_symbols(
        &mut self,
        id: RequestId,
        params: WorkspaceSymbolParams,
    ) -> Result<()> {
        let view = self.open.view();
        self.queue(id, |id, serial, sources| {
            SourceJob::Symbols(SymbolsRead {
                id,
                serial,
                sources,
                view,
                query: params.query,
            })
        })
    }

    pub(super) fn rename(&mut self, id: RequestId, params: RenameFilesParams) -> Result<()> {
        let files: Vec<(PathBuf, PathBuf)> = params
            .files
            .into_iter()
            .filter_map(|file| {
                Some((
                    crate::uri::to_site_path(&file.old_uri).ok()?,
                    crate::uri::to_site_path(&file.new_uri).ok()?,
                ))
            })
            .collect();
        let view = self.open.view();
        self.queue(id, |id, serial, sources| {
            SourceJob::Rename(RenameRead {
                id,
                serial,
                sources,
                view,
                files,
            })
        })
    }

    pub(super) fn enqueue_analysis(
        &mut self,
        id: RequestId,
        src: tola_typst::typst::syntax::Source,
        task: AnalysisTask,
    ) -> Result<()> {
        let uri = match &task {
            AnalysisTask::Query { query, .. } => query.document().0,
            AnalysisTask::Tokens(read) => &read.uri,
        };
        let view = self.open.view();
        let doc_rev = view.get(uri).map(|parsed| (uri.clone(), parsed.revision));
        let root = self.workspace().root.clone();
        let pkg_locs = self.package_locations();
        let boundary = self.source_boundary();
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.requests
            .get_mut(&id)
            .expect("admitted analysis")
            .document_revision = doc_rev;
        self.analyses.push_back(AnalysisRequest {
            id,
            serial: admission.serial,
            sources: self.source_inputs(root, admission.cancellation),
            source_root: self.source_root(),
            view,
            source: src,
            package_locations: pkg_locs,
            boundary,
            task,
        });
        Ok(())
    }

    pub(super) fn query(&mut self, id: RequestId, query: SourceQuery) -> Result<()> {
        let ws = self.workspace();
        let root = ws.root.clone();
        let routes_as_hints = ws.routes_as_hints;
        if let Some(reply) = self.configuration_query(&query, &root) {
            return self.source_reply(id, reply);
        }
        if let SourceQuery::CodeActions(params) = query {
            return self.code_actions(id, params);
        }
        let (uri, pos) = query.document();
        if let Some(src) = self.source(uri) {
            let Ok(cursor) = crate::position::byte_offset(src.lines(), pos) else {
                return self.source_reply(id, crate::query::unavailable(&query));
            };
            if matches!(query, SourceQuery::Completion(_))
                && let Some(reply) = crate::query::source_completion_reply(&src, cursor)
            {
                return self.source_reply(id, reply);
            }
            let name_query = matches!(
                query,
                SourceQuery::Definition(_)
                    | SourceQuery::PrepareRename(_)
                    | SourceQuery::DocumentHighlights(_)
                    | SourceQuery::References(_)
                    | SourceQuery::Rename(_)
            );
            if name_query
                || (matches!(query, SourceQuery::Completion(_))
                    && (is_untitled(uri)
                        || !self
                            .checked
                            .is_some_and(|check| check.revision == self.revision)))
            {
                return self.enqueue_analysis(id, src, AnalysisTask::Query { query, cursor });
            }
            match crate::query::source_reply(&src, &query) {
                Ok(Some(reply)) => return self.source_reply(id, reply),
                Err(err) => {
                    return self
                        .reply(failed_response(id, crate::compiler::SourceFailure::of(err)));
                }
                Ok(None) => {}
            }
        }
        if is_untitled(uri) {
            return self.source_reply(id, crate::query::unavailable(&query));
        }
        let view = self.open.view();
        let check_progress = self.check_progress();
        self.queue(id, |id, serial, sources| {
            SourceJob::Query(QueryRequest {
                id,
                serial,
                sources,
                view,
                routes_as_hints,
                check_progress,
                query,
            })
        })
    }

    /// Stored diagnostics for `uri` when synced version matches, used for clients without `data` support.
    pub(super) fn published_diagnostics(&self, uri: &Uri) -> Option<Vec<lsp_types::Diagnostic>> {
        let Phase::Ready(ws) = &self.phase else {
            return None;
        };
        let synced = self.open.version(uri.as_str());
        ws.diagnostics
            .cause_diagnostics(uri, synced)
            .map(<[lsp_types::Diagnostic]>::to_vec)
    }

    pub(super) fn package_source(
        &mut self,
        id: RequestId,
        params: PackageSourceParams,
    ) -> Result<()> {
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries
            .push_back(SourceJob::PackageSource(PackageSourceRead {
                id,
                serial: admission.serial,
                uri: params.uri,
                cancellation: admission.cancellation,
            }));
        Ok(())
    }

    pub(super) fn incoming_calls(
        &mut self,
        id: RequestId,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<()> {
        let node = params
            .item
            .data
            .as_ref()
            .and_then(|data| crate::call_hierarchy::item_file(data, &self.source_root()));
        let src = node.and_then(|node| self.source(&params.item.uri).filter(|s| s.id() == node));
        let Some(src) = src else {
            return self.source_reply(id, SourceReply::IncomingCalls(None));
        };
        let view = self.open.view();
        self.queue(id, |id, serial, sources| {
            SourceJob::IncomingCalls(IncomingCallsRead {
                id,
                serial,
                sources,
                view,
                source: src,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        dynamic_import_position, finish_analysis, last_position, open_document, open_draft, ready,
        ready_at, references_request, response,
    };
    use lsp_server::Request;
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::time::Instant;
    use tola_typst::typst::syntax::Source;

    #[test]
    fn unnamed_highlights_local() {
        let mut conn = ready(Vec::new());
        let text = "#let count = 1\n#count";
        let uri = open_draft(&mut conn, text);
        let src = Source::detached(text);
        let pos = last_position(text, "count");
        conn.request(Request::new(
            1.into(),
            request::DocumentHighlightRequest::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str()},"position":pos}),
        ))
        .unwrap();
        finish_analysis(&mut conn);
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut conn).response_result.unwrap()).unwrap();
        let expected: Vec<_> = text
            .match_indices("count")
            .map(|(start, name)| {
                crate::position::utf16_range(src.lines(), start..start + name.len()).unwrap()
            })
            .collect();
        assert_eq!(
            highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
            expected
        );
        assert!(conn.next_job(Instant::now()).is_none());
    }

    #[test]
    fn dynamic_references_reach_compiler() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = ready_at(Vec::new(), dir.path().to_path_buf());
        let (uri, pos) = dynamic_import_position(&mut conn, dir.path());
        conn.request(references_request(&uri, pos)).unwrap();
        let pending = conn.next_analysis().expect("source analysis admission");
        let serial = pending.serial;
        conn.completed(crate::compiler::analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            pending,
        ))
        .unwrap();
        assert!(conn.writer.is_empty());
        let Some(SourceJob::Query(req)) = conn.next_job(Instant::now()) else {
            panic!("the compiler lane owns the continuation");
        };
        assert_eq!(req.id, 8.into());
        assert_eq!(req.serial, serial);
        assert!(matches!(req.query, SourceQuery::References(_)));
        assert!(conn.next_job(Instant::now()).is_none());
    }

    #[test]
    fn dynamic_navigation_reaches_compiler() {
        for (method, matches_query) in [
            (
                request::GotoDefinition::METHOD,
                (|query: &SourceQuery| matches!(query, SourceQuery::Definition(_)))
                    as fn(&SourceQuery) -> bool,
            ),
            (
                request::DocumentHighlightRequest::METHOD,
                (|query: &SourceQuery| matches!(query, SourceQuery::DocumentHighlights(_)))
                    as fn(&SourceQuery) -> bool,
            ),
            (
                request::PrepareRenameRequest::METHOD,
                (|query: &SourceQuery| matches!(query, SourceQuery::PrepareRename(_)))
                    as fn(&SourceQuery) -> bool,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut conn = ready_at(Vec::new(), dir.path().to_path_buf());
            let (uri, pos) = dynamic_import_position(&mut conn, dir.path());
            conn.request(Request::new(
                8.into(),
                method.into(),
                json!({"textDocument":{"uri":uri.as_str()},"position":pos}),
            ))
            .unwrap();
            assert!(conn.writer.is_empty());
            finish_analysis(&mut conn);
            let Some(SourceJob::Query(req)) = conn.next_job(Instant::now()) else {
                panic!("the compiler lane owns {method}");
            };
            assert_eq!(req.id, 8.into());
            assert!(
                matches_query(&req.query),
                "{method} arrived as {:?}",
                req.query
            );
            assert!(conn.next_job(Instant::now()).is_none());
        }
    }

    #[test]
    fn static_highlights_answer_names_lane() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = ready_at(Vec::new(), dir.path().to_path_buf());
        let uri = crate::uri::from_file_path(&dir.path().join("page.typ")).unwrap();
        let text = "#let count = 1\n#count\n";
        open_document(&mut conn, &uri, 1, text);
        assert!(matches!(
            conn.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let pos = last_position(text, "count");
        conn.request(Request::new(
            8.into(),
            request::DocumentHighlightRequest::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str()},"position":pos}),
        ))
        .unwrap();
        finish_analysis(&mut conn);
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut conn).response_result.unwrap()).unwrap();
        assert_eq!(highlights.len(), 2, "{highlights:?}");
        assert!(conn.next_job(Instant::now()).is_none());
    }
}
