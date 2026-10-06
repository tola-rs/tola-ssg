//! The requests this connection queues to the compiler lane: routes, lenses, workspace symbols,
//! renames, package sources, incoming calls, and the semantic queries that need the checked world.
//! `query` answers the configuration document and the source-only name graph itself; the remainder
//! lands here, and `published_diagnostics` reads back the causes the last check reported.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use lsp_server::RequestId;
use lsp_types::{
    CallHierarchyIncomingCallsParams, CodeLensParams, RenameFilesParams, Uri, WorkspaceSymbolParams,
};
use tola_build::diagnostic::Diagnostic;

use crate::compiler::{
    AnalysisRequest, IncomingCallsRead, LensesRead, PackageSourceRead, QueryRequest, RenameRead,
    RouteIndexRead, RouteRead, SourceJob, SymbolsRead,
};
use crate::protocol::{self, PackageSourceParams, RouteParams, SourceQuery, SourceReply};

use super::local_requests::is_untitled;
use super::replies::failed_response;
use super::{Connection, Phase};

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// Routes realized from the current source check, not proof of dev-server publication.
    pub(super) fn route(&mut self, id: RequestId, parameters: RouteParams) -> Result<()> {
        if is_untitled(&parameters.uri) {
            return self.source_reply(id, SourceReply::Routes(protocol::RouteReply::default()));
        }
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries.push_back(SourceJob::Route(RouteRead {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            uri: parameters.uri,
        }));
        Ok(())
    }

    /// Every page the checked sources realize, which no request of one document names.
    pub(super) fn route_index(&mut self, id: RequestId) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries
            .push_back(SourceJob::RouteIndex(RouteIndexRead {
                id,
                serial,
                sources: self.source_inputs(root, cancellation),
            }));
        Ok(())
    }

    /// The commands every page of the requested source has.
    pub(super) fn code_lens(&mut self, id: RequestId, parameters: CodeLensParams) -> Result<()> {
        if is_untitled(&parameters.text_document.uri) {
            return self.source_reply(id, SourceReply::CodeLenses(None));
        }
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries.push_back(SourceJob::Lenses(LensesRead {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            uri: parameters.text_document.uri,
        }));
        Ok(())
    }

    /// The symbols of every source the site holds whose name answers the search.
    pub(super) fn workspace_symbols(
        &mut self,
        id: RequestId,
        parameters: WorkspaceSymbolParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries.push_back(SourceJob::Symbols(SymbolsRead {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            query: parameters.query,
        }));
        Ok(())
    }

    /// The edit that keeps every import pointing at a file the editor is renaming.
    pub(super) fn rename(&mut self, id: RequestId, parameters: RenameFilesParams) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
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
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries.push_back(SourceJob::Rename(RenameRead {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            files,
        }));
        Ok(())
    }

    pub(super) fn query(&mut self, id: RequestId, query: SourceQuery) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let package_sources = workspace.package_sources.clone();
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
            let view = self.open.view();
            let reverse_lookup =
                matches!(query, SourceQuery::References(_) | SourceQuery::Rename(_));
            let source_name = reverse_lookup
                && view
                    .get(uri)
                    .map(|snapshot| snapshot.names())
                    .unwrap_or_else(|| {
                        Arc::new(tola_typst_syntax::names::SourceNames::new(source.clone()))
                    })
                    .occurrence(cursor)
                    .is_some();
            if source_name && !view.is_unnamed(source.id()) {
                let package_locations = self.package_locations();
                let boundary = self.source_boundary(&root);
                let Some((serial, cancellation)) = self.admit(&id)? else {
                    return Ok(());
                };
                self.analyses.push_back(AnalysisRequest {
                    id,
                    serial,
                    root,
                    view,
                    source,
                    cursor,
                    package_locations,
                    boundary,
                    query,
                    source_revision: self.open.revision(),
                    cancellation,
                });
                return Ok(());
            }
            let source_reply = if source_name
                || matches!(
                    query,
                    SourceQuery::Definition(_)
                        | SourceQuery::PrepareRename(_)
                        | SourceQuery::DocumentHighlights(_)
                ) {
                let locations = self.package_locations();
                let boundary = self.source_boundary(&root);
                self.source_graph(&root, &source).and_then(|graph| {
                    // A spelling the source-only graph cannot bind — an item or wildcard of a
                    // dynamic import — answers from the compiler lane instead.
                    if matches!(
                        query,
                        SourceQuery::Definition(_)
                            | SourceQuery::PrepareRename(_)
                            | SourceQuery::DocumentHighlights(_)
                    ) && !crate::analysis::unproven(&graph, &source, cursor, &query).is_empty()
                    {
                        return Ok(None);
                    }
                    crate::analysis::respond(
                        &graph,
                        &source,
                        cursor,
                        &query,
                        &root,
                        locations.as_ref(),
                        &boundary,
                    )
                })
            } else {
                crate::query::source_reply(&source, cursor, &query)
            };
            match source_reply {
                Ok(Some(mut reply)) => {
                    let client_root = crate::uri::ClientRoot::new(&root);
                    if let (SourceReply::Definition(Some(definition)), Some(directory)) =
                        (&mut reply, &package_sources)
                        && let Err(error) = crate::query::package_source_definition(
                            definition,
                            directory,
                            &client_root,
                            &self.shutdown.token(),
                        )
                    {
                        return self.reply(failed_response(id, error));
                    }
                    return self.source_reply(id, reply);
                }
                Err(error) => return self.reply(failed_response(id, error)),
                Ok(None) => {}
            }
        }
        if is_untitled(uri) {
            return self.source_reply(id, crate::query::unavailable(&query));
        }
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries.push_back(SourceJob::Query(QueryRequest {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            view: self.open.view(),
            package_sources,
            routes_as_hints,
            check_progress: self.check_progress(),
            query,
        }));
        Ok(())
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
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries
            .push_back(SourceJob::PackageSource(PackageSourceRead {
                id,
                serial,
                uri: parameters.uri,
                cancellation,
            }));
        Ok(())
    }

    /// The sources that reach the client's item, answered from the compiler lane's compilation.
    pub(super) fn incoming_calls(
        &mut self,
        id: RequestId,
        parameters: CallHierarchyIncomingCallsParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let node = parameters
            .item
            .data
            .as_ref()
            .and_then(|data| crate::call_hierarchy::item_file(data, &root));
        let source = node.and_then(|node| {
            self.source(&parameters.item.uri)
                .filter(|source| source.id() == node)
        });
        let Some(source) = source else {
            return self.source_reply(id, SourceReply::IncomingCalls(None));
        };
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.queries
            .push_back(SourceJob::IncomingCalls(IncomingCallsRead {
                id,
                serial,
                sources: self.source_inputs(root, cancellation),
                view: self.open.view(),
                source,
            }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        dynamic_import_position, open_document, open_draft, ready, ready_at, references_request,
        response,
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
        let byte = text.rfind("count").unwrap();
        let position = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        connection
            .request(Request::new(
                1.into(),
                request::DocumentHighlightRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"position":position}),
            ))
            .unwrap();
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
    fn dynamic_import_references_continue_in_the_compiler_lane() {
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
    fn dynamic_import_editor_requests_reach_the_compiler_lane() {
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
    fn static_highlights_answer_in_the_names_lane() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let uri = crate::uri::from_file_path(&directory.path().join("page.typ")).unwrap();
        let text = "#let count = 1\n#count\n";
        open_document(&mut connection, &uri, 1, text);
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let source = Source::detached(text);
        let byte = text.rfind("count").unwrap();
        let position = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        connection
            .request(Request::new(
                8.into(),
                request::DocumentHighlightRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"position":position}),
            ))
            .unwrap();
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        assert_eq!(highlights.len(), 2, "{highlights:?}");
        assert!(connection.next_job(Instant::now()).is_none());
    }
}
