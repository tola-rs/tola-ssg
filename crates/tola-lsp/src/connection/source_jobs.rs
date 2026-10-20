//! Dispatches source requests to analysis or compilation while preserving each request's identity.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use lsp_server::RequestId;
use lsp_types::{
    CallHierarchyIncomingCallsParams, CodeLensParams, RenameFilesParams, Uri, WorkspaceSymbolParams,
};
use tola_build::diagnostic::Diagnostic;

use crate::compiler::{
    AnalysisRequest, AnalysisTask, IncomingCallsRead, LensesRead, PackageSourceRead, QueryRequest,
    RenameRead, RouteIndexRead, RouteRead, SourceJob, SymbolsRead,
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
        let document_revision = view
            .get(uri)
            .map(|snapshot| (uri.clone(), snapshot.revision));
        let root = self.workspace().root.clone();
        let package_sources = self.workspace().package_sources.clone();
        let package_locations = self.package_locations();
        let boundary = self.source_boundary();
        let Some((serial, cancellation)) = self.admit(&id)? else {
            return Ok(());
        };
        self.requests
            .get_mut(&id)
            .expect("admitted analysis")
            .document_revision = document_revision;
        self.analyses.push_back(AnalysisRequest {
            id,
            serial,
            sources: self.source_inputs(root, cancellation),
            source_root: self.source_root(),
            view,
            source,
            package_locations,
            boundary,
            task,
            package_sources,
        });
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
            if matches!(query, SourceQuery::Completion(_))
                && let Some(reply) = crate::query::source_completion_reply(&source, cursor)
            {
                return self.source_reply(id, reply);
            }
            let source_analysis = matches!(
                query,
                SourceQuery::Definition(_)
                    | SourceQuery::PrepareRename(_)
                    | SourceQuery::DocumentHighlights(_)
                    | SourceQuery::References(_)
                    | SourceQuery::Rename(_)
            );
            if source_analysis
                || (matches!(query, SourceQuery::Completion(_))
                    && (is_untitled(uri)
                        || !self
                            .checked
                            .is_some_and(|(revision, _)| revision == self.revision)))
            {
                return self.enqueue_analysis(id, source, AnalysisTask::Query { query, cursor });
            }
            let source_reply = crate::query::source_reply(&source, cursor, &query);
            match source_reply {
                Ok(Some(reply)) => return self.source_reply(id, reply),
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
            .and_then(|data| crate::call_hierarchy::item_file(data, &self.source_root()));
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
    use lsp_types::CompletionResponse;
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;
    use tola_typst::typst::syntax::Source;

    fn finish_analysis(connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
        let request = connection.next_analysis().expect("queued analysis");
        connection
            .completed(crate::compiler::analyze(
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                request,
            ))
            .unwrap();
    }

    /// The client's aliased root, where the platform has symlinks to make one.
    #[cfg(unix)]
    fn client_alias(root: &std::path::Path, content: &std::path::Path) -> Option<PathBuf> {
        let alias = root.join("client");
        std::os::unix::fs::symlink(content, &alias).unwrap();
        Some(alias)
    }

    #[cfg(not(unix))]
    fn client_alias(_: &std::path::Path, _: &std::path::Path) -> Option<PathBuf> {
        None
    }

    #[test]
    fn configured_root_preserves_client_locations() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        std::fs::write(root.join("tola.toml"), "").unwrap();
        std::fs::write(root.join("lib.typ"), "#let marker = 1\n").unwrap();
        std::fs::write(content.join("lib.typ"), "#let marker = 2\n").unwrap();
        std::fs::write(content.join("local.typ"), "#let nearby = 3\n").unwrap();
        let text = "#import \"/lib.typ\": marker\n#import \"local.typ\": nearby\n#marker\n#nearby";
        std::fs::write(content.join("page.typ"), text).unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/page.typ\"]",
        )
        .unwrap();
        let alias = client_alias(&root, &content);
        let workspaces = std::iter::once(content).chain(alias).collect::<Vec<_>>();
        for workspace in workspaces {
            let mut connection = Connection::new(
                Vec::new(),
                |_: &[Diagnostic]| {},
                tola_build::cancellation::BuildCanceller::new(),
                tola_build::BuildResources::default(),
                Some(root.join("tola.toml")),
                None,
                &[],
            );
            connection
                .request(Request::new(
                    1.into(),
                    request::Initialize::METHOD.into(),
                    json!({"rootUri":crate::uri::from_file_path(&workspace).unwrap(),"capabilities":{}}),
                ))
                .unwrap();
            response(&mut connection);
            let _ = connection
                .notification(lsp_server::Notification::new(
                    "initialized".into(),
                    json!({}),
                ))
                .unwrap();
            let uri = crate::uri::from_file_path(&workspace.join("page.typ")).unwrap();
            open_document(&mut connection, &uri, 1, text);
            assert_eq!(
                connection
                    .source(&uri)
                    .unwrap()
                    .id()
                    .vpath()
                    .get_without_slash(),
                "content/page.typ"
            );
            let compiler_root = root.clone();
            let mut compiler = crate::compiler::SourceCompiler::new(
                move |_: &std::path::Path, _: &[(PathBuf, Arc<str>)]| {
                    crate::server::tests::load_configuration(&compiler_root)
                },
                tola_build::BuildResources::default(),
            );
            for warm in [false, true] {
                if warm {
                    let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
                    connection.completed(checked).unwrap();
                    assert_eq!(connection.checked, Some((connection.revision, true)));
                }
                for (line, target) in [(2, root.join("lib.typ")), (3, workspace.join("local.typ"))]
                {
                    connection
                        .request(Request::new(
                            2.into(),
                            request::GotoDefinition::METHOD.into(),
                            json!({"textDocument":{"uri":uri.as_str()},"position":{"line":line,"character":3}}),
                        ))
                        .unwrap();
                    finish_analysis(&mut connection);
                    let definition = response(&mut connection).response_result.unwrap();
                    assert_eq!(
                        definition["uri"],
                        crate::uri::from_file_path(&target).unwrap().as_str()
                    );
                }
                connection
                    .request(Request::new(
                        3.into(),
                        request::References::METHOD.into(),
                        json!({"textDocument":{"uri":uri.as_str()},"position":{"line":2,"character":3},"context":{"includeDeclaration":true}}),
                    ))
                    .unwrap();
                let analysis = connection.next_analysis().unwrap();
                connection
                    .completed(crate::compiler::analyze(
                        &mut crate::analysis::DiskSources::default(),
                        &mut crate::analysis::GraphCache::default(),
                        analysis,
                    ))
                    .unwrap();
                let references = response(&mut connection).response_result.unwrap();
                let targets = references
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|location| location["uri"].as_str().unwrap().to_owned())
                    .collect::<std::collections::BTreeSet<_>>();
                assert_eq!(
                    targets,
                    [
                        uri.as_str().to_owned(),
                        crate::uri::from_file_path(&root.join("lib.typ"))
                            .unwrap()
                            .as_str()
                            .to_owned()
                    ]
                    .into_iter()
                    .collect()
                );

                connection
                    .request(Request::new(
                        4.into(),
                        request::CallHierarchyPrepare::METHOD.into(),
                        json!({"textDocument":{"uri":uri.as_str()},"position":{"line":0,"character":12}}),
                    ))
                    .unwrap();
                let hierarchy = response(&mut connection).response_result.unwrap();
                assert_eq!(
                    hierarchy[0]["uri"],
                    crate::uri::from_file_path(&root.join("lib.typ"))
                        .unwrap()
                        .as_str()
                );
            }
        }
    }

    #[test]
    fn completion_answers_before_first_check() {
        for (marked, expected) in [
            (
                "/// The response.\n#let answer(value) = value\n#ans|",
                Some("answer"),
            ),
            ("#math.eq|", Some("equation")),
            ("#import \"library.typ\": exp|", Some("exported")),
            ("// ordinary comment|", None),
            ("`raw te|xt`", None),
            ("#let label = \"te|xt\"", None),
        ] {
            let cursor = marked.find('|').unwrap();
            let text = marked.replacen('|', "", 1);
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(
                directory.path().join("library.typ"),
                "/// The exported function.\n#let exported(value) = value\n",
            )
            .unwrap();
            let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
            let uri = crate::uri::from_file_path(&directory.path().join("page.typ")).unwrap();
            open_document(&mut connection, &uri, 1, &text);
            let source = Source::detached(text.clone());
            let position = crate::position::utf16_range(source.lines(), cursor..cursor)
                .unwrap()
                .start;
            connection
                .request(Request::new(
                    8.into(),
                    request::Completion::METHOD.into(),
                    json!({"textDocument":{"uri":uri.as_str()},"position":position}),
                ))
                .unwrap();
            let analysis = connection.next_analysis();
            assert_eq!(analysis.is_some(), expected.is_some());
            if let Some(analysis) = analysis {
                assert!(connection.writer.is_empty());
                connection
                    .completed(crate::compiler::analyze(
                        &mut crate::analysis::DiskSources::default(),
                        &mut crate::analysis::GraphCache::default(),
                        analysis,
                    ))
                    .unwrap();
            }
            let completion: CompletionResponse =
                serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
            let items = match completion {
                CompletionResponse::Array(items) => items,
                CompletionResponse::List(list) => list.items,
            };
            if let Some(expected) = expected {
                let selected = items.iter().find(|item| item.label == expected).unwrap();
                assert!(selected.documentation.is_some(), "{expected}: {selected:?}");
            } else {
                assert!(items.is_empty(), "{text}: {items:?}");
            }
            assert!(connection.checked.is_none());
            assert!(matches!(
                connection.next_job(Instant::now()),
                Some(SourceJob::Check(_))
            ));
            assert!(connection.next_job(Instant::now()).is_none());
        }
    }

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
        finish_analysis(&mut connection);
        let highlights: Vec<lsp_types::DocumentHighlight> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        assert_eq!(highlights.len(), 2, "{highlights:?}");
        assert!(connection.next_job(Instant::now()).is_none());
    }
}
