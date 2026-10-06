//! The requests this connection answers without a check: a source's formatting, folds, outline,
//! links, tokens, and selection, the on-enter edits in it, the call-hierarchy steps that read one
//! source, the configuration document's answers and the diagnostics pulled from the last check, and
//! the source reads they all begin from. Requests that need the checked world are queued by
//! `source_jobs`.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use lsp_server::{ErrorCode, RequestId, Response};
use lsp_types::{
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, CompletionResponse,
    DiagnosticServerCancellationData, DocumentDiagnosticParams, DocumentFormattingParams,
    DocumentLinkParams, DocumentRangeFormattingParams, DocumentSymbolParams, FoldingRangeParams,
    SelectionRangeParams, SemanticTokensDeltaParams, SemanticTokensFullDeltaResult,
    SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensResult, Uri,
};
use tola_build::diagnostic::Diagnostic;
use tola_typst::typst::syntax::Source;

use crate::diagnostic::Pulled;
use crate::protocol::{self, EnterParams, EnterReply, SourceQuery, SourceReply};

use super::Connection;
use super::replies::failed_response;

/// Whether this URI names a document the editor holds unsaved.
pub(super) fn is_untitled(uri: &Uri) -> bool {
    uri.as_str().starts_with("untitled:")
}
impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn source_boundary(&self, root: &Path) -> tola_typst::SourceBoundary {
        self.configuration.as_ref().map_or_else(
            || {
                crate::sources::base_source_boundary(
                    root,
                    self.resources.input_scope() == tola_build::InputScope::Pure,
                )
            },
            |config| self.resources.source_boundary(config),
        )
    }

    pub(super) fn package_locations(&self) -> Option<tola_typst::PackageLocations> {
        self.configuration
            .as_ref()
            .map(|config| self.resources.package_locations(config))
    }

    /// The source-only name graph over one source, as this connection's own answers read it.
    pub(super) fn source_graph(
        &mut self,
        root: &Path,
        source: &Source,
    ) -> Result<Arc<tola_typst_syntax::imports::NameGraph>> {
        let locations = self.package_locations();
        let boundary = self.source_boundary(root);
        let view = self.open.view();
        crate::analysis::source_only_graph(
            root,
            &view,
            source,
            locations.as_ref(),
            &boundary,
            false,
            &mut self.disk,
            Some(crate::analysis::CachedGraph {
                cache: &mut self.graphs,
                revision: self.open.revision(),
            }),
            &self.shutdown.token(),
        )
    }

    pub(super) fn source(&self, uri: &Uri) -> Option<Source> {
        let workspace = self.workspace();
        let boundary = self.source_boundary(&workspace.root);
        if let Ok(path) = crate::uri::to_site_path(uri.as_str()) {
            boundary.check(&path).ok()?;
        }
        self.open
            .source(uri)
            .cloned()
            .or_else(|| crate::sources::load(uri, &workspace.root, &self.overrides, &boundary))
    }

    fn text_request<T>(
        &self,
        uri: &Uri,
        answer: impl FnOnce(&Source, &Path) -> Option<T>,
    ) -> Option<T> {
        let workspace = self.workspace();
        self.source(uri)
            .and_then(|source| answer(&source, &workspace.root))
    }

    pub(super) fn formatting(
        &mut self,
        id: RequestId,
        parameters: DocumentFormattingParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let formatter = workspace.formatter;
        let _ = &parameters;
        let edits = self.text_request(&parameters.text_document.uri, |source, _| {
            crate::formatting::edits(source.text(), &parameters.options, &formatter)
        });
        self.reply(Response::new_ok(id, edits))
    }

    /// The edits that format one range of a source, which leaves the rest as the author wrote it.
    pub(super) fn range_formatting(
        &mut self,
        id: RequestId,
        parameters: DocumentRangeFormattingParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let formatter = workspace.formatter;
        let edits = self.text_request(&parameters.text_document.uri, |source, _| {
            let lines = source.lines();
            let bytes = crate::position::byte_offset(lines, parameters.range.start).ok()?
                ..crate::position::byte_offset(lines, parameters.range.end).ok()?;
            crate::formatting::range_edits(source.text(), bytes, &parameters.options, &formatter)
        });
        self.reply(Response::new_ok(id, edits))
    }

    pub(super) fn folding(&mut self, id: RequestId, parameters: FoldingRangeParams) -> Result<()> {
        let workspace = self.workspace();
        let line_folding_only = workspace.line_folding_only;
        let ranges = self.text_request(&parameters.text_document.uri, |source, _| {
            crate::folding::ranges(source, line_folding_only)
        });
        self.reply(Response::new_ok(id, ranges))
    }

    pub(super) fn symbols(
        &mut self,
        id: RequestId,
        parameters: DocumentSymbolParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let hierarchical = workspace.features.hierarchical_symbols;
        let uri = &parameters.text_document.uri;
        let symbols = self.text_request(uri, |source, _| {
            crate::symbols::outline(source, uri, hierarchical)
        });
        self.reply(Response::new_ok(id, symbols))
    }

    pub(super) fn selection(
        &mut self,
        id: RequestId,
        parameters: SelectionRangeParams,
    ) -> Result<()> {
        let Some(source) = self.source(&parameters.text_document.uri) else {
            return self.reply(Response::new_ok(
                id,
                Option::<Vec<lsp_types::SelectionRange>>::None,
            ));
        };
        match crate::selection::ranges(&source, &parameters.positions) {
            Some(ranges) => self.reply(Response::new_ok(id, ranges)),
            None => self.reject(
                id,
                ErrorCode::InvalidParams,
                "selection position is outside the source",
            ),
        }
    }

    pub(super) fn links(&mut self, id: RequestId, parameters: DocumentLinkParams) -> Result<()> {
        if is_untitled(&parameters.text_document.uri) {
            return self.reply(Response::new_ok(
                id,
                Option::<Vec<lsp_types::DocumentLink>>::None,
            ));
        }
        let links = self.text_request(&parameters.text_document.uri, crate::links::paths);
        self.reply(Response::new_ok(id, links))
    }

    pub(super) fn enter(&mut self, id: RequestId, parameters: EnterParams) -> Result<()> {
        let edits = self.text_request(&parameters.uri, |source, _| {
            crate::on_enter::edits(source, parameters.position)
        });
        self.reply(Response::new_ok(id, EnterReply { edits }))
    }

    fn current_tokens(&mut self, uri: &Uri) -> Result<Option<lsp_types::SemanticTokens>> {
        let workspace = self.workspace();
        let Some(source) = self.source(uri) else {
            return Ok(None);
        };
        let root = workspace.root.clone();
        let graph = self.source_graph(&root, &source)?;
        Ok(Some(crate::tokens::tokens(&source, &graph)))
    }

    fn remember_tokens(&mut self, uri: Uri, mut tokens: lsp_types::SemanticTokens) {
        self.tokens_serial += 1;
        tokens.result_id = Some(self.tokens_serial.to_string());
        self.tokens.insert(uri, tokens);
    }

    pub(super) fn tokens_delta(
        &mut self,
        id: RequestId,
        parameters: SemanticTokensDeltaParams,
    ) -> Result<()> {
        let uri = parameters.text_document.uri;
        let current = match self.current_tokens(&uri) {
            Ok(Some(tokens)) => tokens,
            Ok(None) => {
                return self.reply(Response::new_ok(
                    id,
                    Option::<SemanticTokensFullDeltaResult>::None,
                ));
            }
            Err(error) => return self.reply(failed_response(id, error)),
        };
        let delta = self
            .tokens
            .get(&uri)
            .filter(|previous| {
                previous.result_id.as_deref() == Some(parameters.previous_result_id.as_str())
            })
            .map(|previous| crate::tokens::delta(previous, &current));
        self.remember_tokens(uri.clone(), current);
        let current = self.tokens.get(&uri).expect("current tokens were retained");
        let reply = match delta {
            Some(mut delta) => {
                delta.result_id.clone_from(&current.result_id);
                Response::new_ok(id, delta)
            }
            None => Response::new_ok(id, current),
        };
        self.reply(reply)
    }

    pub(super) fn tokens(&mut self, id: RequestId, parameters: SemanticTokensParams) -> Result<()> {
        let uri = parameters.text_document.uri;
        let tokens = match self.current_tokens(&uri) {
            Ok(Some(tokens)) => tokens,
            Ok(None) => {
                return self.reply(Response::new_ok(id, Option::<SemanticTokensResult>::None));
            }
            Err(error) => return self.reply(failed_response(id, error)),
        };
        self.remember_tokens(uri.clone(), tokens);
        self.reply(Response::new_ok(id, self.tokens.get(&uri)))
    }

    pub(super) fn tokens_range(
        &mut self,
        id: RequestId,
        parameters: SemanticTokensRangeParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let Some(source) = self.source(&parameters.text_document.uri) else {
            return self.reply(Response::new_ok(
                id,
                Option::<lsp_types::SemanticTokens>::None,
            ));
        };
        let root = workspace.root.clone();
        let graph = self.source_graph(&root, &source);
        match graph {
            Ok(graph) => self.reply(Response::new_ok(
                id,
                Some(crate::tokens::in_range(&source, &graph, parameters.range)),
            )),
            Err(error) => self.reply(failed_response(id, error)),
        }
    }

    /// The configuration document this workspace resolves.
    ///
    /// It is the file the last completed check resolved, and the file Tola looks for at the
    /// workspace root until one has: an author may name the configuration with `--config` or reach
    /// it through a symlink, so only the resolved path identifies it.
    pub(super) fn configuration_path(&self, root: &Path) -> PathBuf {
        let config = self
            .configuration
            .as_ref()
            .map(|config| config.config_path().to_path_buf())
            .or_else(|| self.named_configuration.clone())
            .unwrap_or_else(|| root.join(tola_build::config::loading::CONFIG_FILE_NAME));
        tola_build::filesystem::normalize_existing_prefix(&config)
    }

    /// The text the editor holds for one document: the buffer it opened, or the file on disk.
    ///
    /// Both spellings of one file find that buffer, because an author may reach a document through
    /// a symlink while the site resolves the file itself. State Tola generates or publishes is not
    /// a document any answer reads.
    pub(super) fn document_text(&self, path: &Path) -> Option<Cow<'_, str>> {
        if self.open.is_generated_path(path) {
            return None;
        }
        let normalized = tola_build::filesystem::normalize_existing_prefix(path);
        let opened = self.overrides.iter().find(|(open, _)| {
            **open == *path || tola_build::filesystem::normalize_existing_prefix(open) == normalized
        });
        match opened {
            Some((_, text)) => Some(Cow::Borrowed(text.as_ref())),
            None => std::fs::read_to_string(path).ok().map(Cow::Owned),
        }
    }

    /// The reply a request for the site's configuration file receives, answered from the
    /// configuration the editor holds.
    ///
    /// Answers `None` for every other document, so a Typst source keeps the one
    /// prepared-compilation path.
    pub(super) fn configuration_query(
        &self,
        query: &SourceQuery,
        root: &Path,
    ) -> Option<SourceReply> {
        let (uri, position) = query.document();
        let requested = crate::uri::to_site_path(uri.as_str()).ok()?;
        let config = self.configuration_path(root);
        if requested != config {
            return None;
        }
        let text = self.document_text(&config)?;
        match query {
            SourceQuery::Completion(_) => Some(SourceReply::Completion(CompletionResponse::Array(
                crate::query::config::document(text.as_ref(), position, self.host_sections),
            ))),
            SourceQuery::Hover(_) => Some(SourceReply::Hover(
                crate::query::config::document_hover(text.as_ref(), position, self.host_sections),
            )),
            SourceQuery::CodeActions(params) => {
                let only = params.context.only.as_deref();
                if !crate::code_actions::admits(only, &lsp_types::CodeActionKind::QUICKFIX) {
                    // A fix-all request reads the document's own diagnostics, which this lane does
                    // not answer, so the request continues to the handler that computes them.
                    if crate::code_actions::only_names(
                        only,
                        &lsp_types::CodeActionKind::SOURCE_FIX_ALL,
                    ) {
                        return None;
                    }
                    return Some(SourceReply::CodeActions(None));
                }
                let edit =
                    crate::query::config::replacement(text.as_ref(), position, self.host_sections)?;
                let title = format!("replace the key with `{}`", edit.new_text);
                Some(SourceReply::CodeActions(Some(vec![protocol::quick_fix(
                    uri, title, edit,
                )])))
            }
            SourceQuery::Definition(_)
            | SourceQuery::SignatureHelp(_)
            | SourceQuery::PrepareRename(_)
            | SourceQuery::Rename(_)
            | SourceQuery::References(_)
            | SourceQuery::DocumentHighlights(_)
            | SourceQuery::DocumentColors(_)
            | SourceQuery::ColorPresentations(_)
            | SourceQuery::InlayHints(_) => None,
        }
    }

    /// The item the cursor names, answered from the editor's own text: an item's range is a file's
    /// start rather than a declaration, so no compilation is read.
    pub(super) fn prepare_call_hierarchy(
        &mut self,
        id: RequestId,
        parameters: CallHierarchyPrepareParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let uri = &parameters.text_document_position_params.text_document.uri;
        let position = parameters.text_document_position_params.position;
        let Some(source) = self.source(uri) else {
            return self.source_reply(id, SourceReply::PrepareCallHierarchy(None));
        };
        let Ok(cursor) = crate::position::byte_offset(source.lines(), position) else {
            return self.source_reply(id, SourceReply::PrepareCallHierarchy(None));
        };
        let boundary = self.source_boundary(&root);
        let item = crate::call_hierarchy::prepare(
            &root,
            self.package_locations().as_ref(),
            &boundary,
            &source,
            cursor,
        );
        self.source_reply(
            id,
            SourceReply::PrepareCallHierarchy(item.map(|item| vec![item])),
        )
    }

    /// The sources the client's item reaches, answered from the item's own text.
    pub(super) fn outgoing_calls(
        &mut self,
        id: RequestId,
        parameters: CallHierarchyOutgoingCallsParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let root = workspace.root.clone();
        let source = self.source(&parameters.item.uri).filter(|source| {
            parameters
                .item
                .data
                .as_ref()
                .and_then(|data| crate::call_hierarchy::item_file(data, &root))
                .is_some_and(|node| node == source.id())
        });
        let Some(source) = source else {
            return self.source_reply(id, SourceReply::OutgoingCalls(None));
        };
        let boundary = self.source_boundary(&root);
        let calls = crate::call_hierarchy::outgoing(
            &root,
            self.package_locations().as_ref(),
            &boundary,
            &source,
        );
        self.source_reply(
            id,
            SourceReply::OutgoingCalls((!calls.is_empty()).then_some(calls)),
        )
    }

    /// The diagnostics one document pulls, answered from the last completed check.
    pub(super) fn diagnostic(
        &mut self,
        id: RequestId,
        parameters: DocumentDiagnosticParams,
    ) -> Result<()> {
        let workspace = self.workspace();
        let uri = parameters.text_document.uri.clone();
        let synced = self.open.version(uri.as_str());
        match workspace
            .diagnostics
            .pull(&uri, parameters.previous_result_id.as_deref(), synced)
        {
            Pulled::Report(report) => self.reply(Response::new_ok(id, report)),
            // A report that predates the text the client has synced is refused in the protocol's
            // own shape, and the client asks again once the site's own state settles.
            Pulled::Stale => {
                let data = serde_json::to_value(DiagnosticServerCancellationData {
                    retrigger_request: true,
                })
                .expect("the cancellation data is a plain struct");
                let mut response = Response::new_err(
                    id,
                    ErrorCode::ServerCancelled as i32,
                    "the diagnostics Tola holds are for an older version of this document"
                        .to_owned(),
                );
                if let Err(error) = &mut response.response_result {
                    error.data = Some(data);
                }
                self.reply(response)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ClientFeatures;
    use crate::compiler::SourceCompilation;
    use crate::compiler::SourceJob;
    use crate::connection::tests::{
        messages, open_draft, ready, ready_at, ready_workspace, response,
    };
    use crate::server::tests::load_configuration;
    use lsp_server::{Message, Notification, Request};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Instant;
    use tola_typst::typst::syntax::Source;

    fn full_tokens(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        uri: &Uri,
        id: i32,
    ) -> lsp_types::SemanticTokens {
        connection
            .request(Request::new(
                id.into(),
                request::SemanticTokensFullRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()}}),
            ))
            .unwrap();
        serde_json::from_value(response(connection).response_result.unwrap()).unwrap()
    }

    fn reply_for(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        id: &RequestId,
    ) -> serde_json::Value {
        messages(connection)
            .into_iter()
            .find_map(|message| match message {
                Message::Response(response) if response.id == *id => {
                    Some(response.response_result.expect("an answer"))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no reply for {id:?}"))
    }

    #[test]
    fn missing_delta_base_returns_full_tokens() {
        let mut connection = ready(Vec::new());
        let uri = open_draft(&mut connection, "#let count = 1\n#count");
        let first = full_tokens(&mut connection, &uri, 1);
        let _ = connection.notification(Notification::new(
            notification::DidChangeTextDocument::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str(),"version":2},"contentChanges":[{"text":"#let count = (1, 2)\n#count"}]}),
        )).unwrap();
        let current = full_tokens(&mut connection, &uri, 2);
        connection
            .request(Request::new(
                3.into(),
                request::SemanticTokensFullDeltaRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"previousResultId":first.result_id}),
            ))
            .unwrap();
        let tokens: SemanticTokensFullDeltaResult =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        let SemanticTokensFullDeltaResult::Tokens(tokens) = tokens else {
            panic!("unknown base must return full tokens");
        };
        assert_eq!(tokens.data, current.data);
        assert_ne!(tokens.data, first.data);
    }

    #[test]
    fn closed_document_forgets_delta_base() {
        let mut connection = ready(Vec::new());
        let uri = open_draft(&mut connection, "#let count = 1\n#count");
        let previous = full_tokens(&mut connection, &uri, 1);
        let _ = connection
            .notification(Notification::new(
                notification::DidCloseTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()}}),
            ))
            .unwrap();
        open_draft(&mut connection, "#let count = 1\n#count");
        connection
            .request(Request::new(
                2.into(),
                request::SemanticTokensFullDeltaRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"previousResultId":previous.result_id}),
            ))
            .unwrap();
        let tokens: SemanticTokensFullDeltaResult =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        assert!(matches!(tokens, SemanticTokensFullDeltaResult::Tokens(_)));
    }

    #[test]
    fn invalid_selection_rejects_whole_request() {
        let mut connection = ready(Vec::new());
        let text = "#let count = 1";
        let uri = open_draft(&mut connection, text);
        let source = Source::detached(text);
        let byte = text.find("count").unwrap();
        let valid = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        let mut invalid = crate::position::utf16_range(source.lines(), text.len()..text.len())
            .unwrap()
            .end;
        invalid.character += 1;
        connection
            .request(Request::new(
                1.into(),
                request::SelectionRangeRequest::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()},"positions":[valid, invalid]}),
            ))
            .unwrap();
        assert_eq!(
            response(&mut connection).response_result.unwrap_err().code,
            ErrorCode::InvalidParams as i32
        );
    }

    /// Both call-hierarchy requests answer over the protocol, and the items they hold address
    /// their files under the workspace root.
    #[test]
    fn call_hierarchy_answers_over_the_request_path() {
        let site = tempfile::tempdir().unwrap();
        let root = site.path().canonicalize().unwrap();
        std::fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();
        let note = root.join("templates/note.typ");
        std::fs::create_dir_all(note.parent().unwrap()).unwrap();
        std::fs::write(&note, "Note.\n").unwrap();
        let page = root.join("content/document.typ");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "#include \"../templates/note.typ\"\nDocument.\n").unwrap();
        std::fs::write(root.join("tola.toml"), "").unwrap();
        let mut connection = ready_at(Vec::new(), root.clone());
        let note_uri = crate::uri::from_file_path(&root.join("templates/note.typ")).unwrap();

        let _ = connection.request(Request::new(
            1.into(),
            request::CallHierarchyPrepare::METHOD.into(),
            json!({
                "textDocument": { "uri": note_uri },
                "position": { "line": 0, "character": 0 }
            }),
        ));
        let prepared = reply_for(&mut connection, &RequestId::from(1));
        let items = prepared.as_array().expect("a prepare answers an array");
        assert_eq!(items.len(), 1, "{prepared}");
        assert_eq!(items[0]["name"], "templates/note.typ");
        assert_eq!(items[0]["uri"], json!(note_uri.as_str()));

        let _ = connection.request(Request::new(
            2.into(),
            request::CallHierarchyIncomingCalls::METHOD.into(),
            json!({ "item": items[0].clone() }),
        ));
        let Some(job @ SourceJob::IncomingCalls(_)) = connection.next_job(Instant::now()) else {
            panic!("the incoming-calls request reaches the compiler lane");
        };
        let mut compiler = crate::compiler::SourceCompiler::new(
            |root: &Path, _: &[(PathBuf, Arc<str>)]| load_configuration(root),
            tola_build::BuildResources::default(),
        );
        let SourceCompilation::Answered { response, .. } = compiler.compile(job) else {
            panic!("an answered job");
        };
        let response = response.expect("the site compiles");
        let SourceReply::IncomingCalls(Some(calls)) = response else {
            panic!("incoming calls: {response:?}");
        };

        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].from.name, "content/document.typ");
        assert_eq!(
            calls[0].from.detail.as_deref(),
            Some("Served at /document/")
        );
        assert_eq!(
            calls[0].from.uri,
            crate::uri::from_file_path(&root.join("content/document.typ")).unwrap()
        );
        assert_eq!(
            calls[0].from_ranges,
            [lsp_types::Range::new(
                lsp_types::Position::new(0, 9),
                lsp_types::Position::new(0, 32)
            )]
        );
    }

    #[test]
    fn configuration_actions_respect_kind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "[site]\ndescriptio = \"kept\"\n").unwrap();
        let uri = crate::uri::from_file_path(&path).unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let workspace = ready_workspace(&mut connection);
        workspace.features = ClientFeatures::new(&serde_json::from_value(json!({
            "textDocument":{"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","refactor","source"]}}}}
        })).unwrap());
        for (kind, wanted) in [("refactor", false), ("source", false), ("quickfix", true)] {
            connection
                .request(Request::new(
                    8.into(),
                    request::CodeActionRequest::METHOD.into(),
                    json!({
                        "textDocument":{"uri":uri.as_str()},
                        "range":{"start":{"line":1,"character":5},"end":{"line":1,"character":5}},
                        "context":{"diagnostics":[],"only":[kind]}
                    }),
                ))
                .unwrap();
            let actions: Option<Vec<lsp_types::CodeActionOrCommand>> =
                serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
            let actions = actions.unwrap_or_default();
            if wanted {
                let [lsp_types::CodeActionOrCommand::CodeAction(action)] = actions.as_slice()
                else {
                    panic!("one applicable key correction");
                };
                let edit = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri][0];
                assert_eq!(edit.new_text, "description");
            } else {
                assert!(actions.is_empty(), "{kind} returned {actions:?}");
            }
        }
    }
}
