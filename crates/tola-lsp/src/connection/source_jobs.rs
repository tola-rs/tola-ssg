//! Dispatches source requests to analysis or compilation while preserving each request's identity.

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
    /// Admit one source request and queue the job that answers it.
    ///
    /// The job carries the identity and source inputs the admission assigned, so a job no admitted
    /// request owns cannot be queued; a refused request queues nothing.
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

    /// Routes realized from the current source check, not proof of dev-server publication.
    pub(super) fn route(&mut self, id: RequestId, parameters: RouteParams) -> Result<()> {
        if is_untitled(&parameters.uri) {
            return self.source_reply(id, SourceReply::Routes(protocol::RouteReply::default()));
        }
        self.queue(id, |id, serial, sources| {
            SourceJob::Route(RouteRead {
                id,
                serial,
                sources,
                uri: parameters.uri,
            })
        })
    }

    /// Every page the checked sources realize, which no request of one document names.
    pub(super) fn route_index(&mut self, id: RequestId) -> Result<()> {
        self.queue(id, |id, serial, sources| {
            SourceJob::RouteIndex(RouteIndexRead {
                id,
                serial,
                sources,
            })
        })
    }

    /// The commands every page of the requested source has.
    pub(super) fn code_lens(&mut self, id: RequestId, parameters: CodeLensParams) -> Result<()> {
        if is_untitled(&parameters.text_document.uri) {
            return self.source_reply(id, SourceReply::CodeLenses(None));
        }
        self.queue(id, |id, serial, sources| {
            SourceJob::Lenses(LensesRead {
                id,
                serial,
                sources,
                uri: parameters.text_document.uri,
            })
        })
    }

    /// The symbols of every source the site holds whose name answers the search.
    pub(super) fn workspace_symbols(
        &mut self,
        id: RequestId,
        parameters: WorkspaceSymbolParams,
    ) -> Result<()> {
        let view = self.open.view();
        self.queue(id, |id, serial, sources| {
            SourceJob::Symbols(SymbolsRead {
                id,
                serial,
                sources,
                view,
                query: parameters.query,
            })
        })
    }

    /// The edit that keeps every import pointing at a file the editor is renaming.
    pub(super) fn rename(&mut self, id: RequestId, parameters: RenameFilesParams) -> Result<()> {
        let files: Vec<(PathBuf, PathBuf)> = parameters
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
        source: tola_typst::typst::syntax::Source,
        task: AnalysisTask,
    ) -> Result<()> {
        let uri = match &task {
            AnalysisTask::Query { query, .. } => query.document().0,
            AnalysisTask::Tokens(read) => &read.uri,
        };
        let view = self.open.view();
        let document_revision = view.get(uri).map(|parsed| (uri.clone(), parsed.revision));
        let root = self.workspace().root.clone();
        let package_locations = self.package_locations();
        let boundary = self.source_boundary();
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.requests
            .get_mut(&id)
            .expect("admitted analysis")
            .document_revision = document_revision;
        self.analyses.push_back(AnalysisRequest {
            id,
            serial: admission.serial,
            sources: self.source_inputs(root, admission.cancellation),
            source_root: self.source_root(),
            view,
            source,
            package_locations,
            boundary,
            task,
        });
        Ok(())
    }

    pub(super) fn query(&mut self, id: RequestId, query: SourceQuery) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let routes_as_hints = workspace.routes_as_hints;
        if let Some(reply) = self.configuration_query(&query, &root) {
            return self.source_reply(id, reply);
        }
        // A fix reads the text the author is looking at, so it answers without the compilation a
        // broken source cannot produce.
        if let SourceQuery::CodeActions(params) = query {
            return self.code_actions(id, params);
        }
        let (uri, position) = query.document();
        if let Some(source) = self.source(uri) {
            let Ok(cursor) = crate::position::byte_offset(source.lines(), position) else {
                return self.source_reply(id, crate::query::unavailable(&query));
            };
            if matches!(query, SourceQuery::Completion(_))
                && let Some(reply) = crate::query::source_completion_reply(&source, cursor)
            {
                return self.source_reply(id, reply);
            }
            let answered_from_names = matches!(
                query,
                SourceQuery::Definition(_)
                    | SourceQuery::PrepareRename(_)
                    | SourceQuery::DocumentHighlights(_)
                    | SourceQuery::References(_)
                    | SourceQuery::Rename(_)
            );
            if answered_from_names
                || (matches!(query, SourceQuery::Completion(_))
                    && (is_untitled(uri)
                        || !self
                            .checked
                            .is_some_and(|check| check.revision == self.revision)))
            {
                return self.enqueue_analysis(id, source, AnalysisTask::Query { query, cursor });
            }
            let source_reply = crate::query::source_reply(&source, &query);
            match source_reply {
                Ok(Some(reply)) => return self.source_reply(id, reply),
                Err(error) => return self.reply(failed_response(id, error)),
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

    /// The causes the last completed check reported for one document, when they are still the
    /// version the client has synced: a client that reads no `data` cannot echo them, so the server
    /// answers its fix requests from these.
    pub(super) fn published_diagnostics(&self, uri: &Uri) -> Option<Vec<lsp_types::Diagnostic>> {
        let Phase::Ready(workspace) = &self.phase else {
            return None;
        };
        let synced = self.open.version(uri.as_str());
        workspace
            .diagnostics
            .cause_diagnostics(uri, synced)
            .map(<[lsp_types::Diagnostic]>::to_vec)
    }

    pub(super) fn package_source(
        &mut self,
        id: RequestId,
        parameters: PackageSourceParams,
    ) -> Result<()> {
        let Some(admission) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries
            .push_back(SourceJob::PackageSource(PackageSourceRead {
                id,
                serial: admission.serial,
                uri: parameters.uri,
                cancellation: admission.cancellation,
            }));
        Ok(())
    }

    /// The sources that reach the client's item, answered from the compiler lane's compilation.
    pub(super) fn incoming_calls(
        &mut self,
        id: RequestId,
        parameters: CallHierarchyIncomingCallsParams,
    ) -> Result<()> {
        let node = parameters
            .item
            .data
            .as_ref()
            .and_then(|data| crate::call_hierarchy::item_file(data, &self.source_root()));
        let source = node.and_then(|node| {
            self.source(&parameters.item.uri)
                .filter(|source| source.id() == node)
        });
        let Some(source) = source else {
            return self.source_reply(id, SourceReply::IncomingCalls(None));
        };
        let view = self.open.view();
        self.queue(id, |id, serial, sources| {
            SourceJob::IncomingCalls(IncomingCallsRead {
                id,
                serial,
                sources,
                view,
                source,
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
    fn unnamed_highlights_need_no_compilation() {
        let mut connection = ready(Vec::new());
        let text = "#let count = 1\n#count";
        let uri = open_draft(&mut connection, text);
        let source = Source::detached(text);
        let position = last_position(text, "count");
        connection
            .request(Request::new(
                1.into(),
                request::DocumentHighlightRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"position":position}),
            ))
            .unwrap();
        finish_analysis(&mut connection);
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        let expected: Vec<_> = text
            .match_indices("count")
            .map(|(start, name)| {
                crate::position::utf16_range(source.lines(), start..start + name.len()).unwrap()
            })
            .collect();
        assert_eq!(
            highlights
                .iter()
                .map(|highlight| highlight.range)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(connection.next_job(Instant::now()).is_none());
    }

    /// The compiler lane finishes what the source-only graph cannot prove, under the client's own
    /// request.
    #[test]
    fn dynamic_references_reach_compiler() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let (uri, position) = dynamic_import_position(&mut connection, directory.path());
        connection
            .request(references_request(&uri, position))
            .unwrap();
        let pending = connection
            .next_analysis()
            .expect("source analysis admission");
        let serial = pending.serial;
        connection
            .completed(crate::compiler::analyze(
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                pending,
            ))
            .unwrap();
        assert!(
            connection.writer.is_empty(),
            "the source lane answers nothing of its own"
        );
        let Some(SourceJob::Query(request)) = connection.next_job(Instant::now()) else {
            panic!("the compiler lane owns the continuation");
        };
        assert_eq!(request.id, 8.into());
        assert_eq!(request.serial, serial);
        assert!(matches!(request.query, SourceQuery::References(_)));
        assert!(connection.next_job(Instant::now()).is_none());
    }

    /// A definition, highlight, or rename preparation the source-only graph cannot bind answers
    /// from the compiler lane, under the same request, instead of from the names lane.
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
            let directory = tempfile::tempdir().unwrap();
            let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
            let (uri, position) = dynamic_import_position(&mut connection, directory.path());
            connection
                .request(Request::new(
                    8.into(),
                    method.into(),
                    json!({"textDocument":{"uri":uri.as_str()},"position":position}),
                ))
                .unwrap();
            assert!(
                connection.writer.is_empty(),
                "{method} answered in the names lane"
            );
            finish_analysis(&mut connection);
            let Some(SourceJob::Query(request)) = connection.next_job(Instant::now()) else {
                panic!("the compiler lane owns {method}");
            };
            assert_eq!(request.id, 8.into());
            assert!(
                matches_query(&request.query),
                "{method} arrived as {:?}",
                request.query
            );
            assert!(connection.next_job(Instant::now()).is_none());
        }
    }

    /// A name the source-only graph binds answers highlights without the compiler lane.
    #[test]
    fn static_highlights_answer_names_lane() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let uri = crate::uri::from_file_path(&directory.path().join("page.typ")).unwrap();
        let text = "#let count = 1\n#count\n";
        open_document(&mut connection, &uri, 1, text);
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let position = last_position(text, "count");
        connection
            .request(Request::new(
                8.into(),
                request::DocumentHighlightRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"position":position}),
            ))
            .unwrap();
        finish_analysis(&mut connection);
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        assert_eq!(highlights.len(), 2, "{highlights:?}");
        assert!(connection.next_job(Instant::now()).is_none());
    }
}
