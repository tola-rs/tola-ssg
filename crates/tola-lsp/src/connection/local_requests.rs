//! The requests this connection answers without a check: a source's formatting, folds, outline,
//! links and selection, the on-enter edits in it, the call-hierarchy steps that read one
//! source, the configuration document's answers and the diagnostics pulled from the last check, and
//! the source reads they all begin from. Requests that need the checked world are queued by
//! `source_jobs`; semantic tokens enqueue their source analysis here.

use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use lsp_server::{ErrorCode, RequestId, Response};
use lsp_types::{
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, CodeActionKind,
    CompletionResponse, DiagnosticServerCancellationData, DocumentDiagnosticParams,
    DocumentFormattingParams, DocumentLinkParams, DocumentRangeFormattingParams,
    DocumentSymbolParams, FoldingRangeParams, SelectionRangeParams, SemanticTokensDeltaParams,
    SemanticTokensParams, SemanticTokensRangeParams, Uri,
};
use tola_build::config::ResolvedSiteConfig;
use tola_diagnostic::Diagnostic;
use tola_typst::typst::syntax::Source;

use crate::compiler::{AnalysisTask, TokensKind, TokensRead};
use crate::diagnostic::Pulled;
use crate::protocol::{self, EnterParams, EnterReply, SourceQuery, SourceReply};

use super::Connection;

/// Whether this URI names a document the editor holds unsaved.
pub(super) fn is_untitled(uri: &Uri) -> bool {
    uri.as_str().starts_with("untitled:")
}
impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// The root the client named, which a reply addresses its documents under.
    pub(super) fn client_root(&self) -> crate::uri::ClientRoot {
        crate::uri::ClientRoot::new(&self.workspace().root)
    }

    pub(super) fn source_boundary(&self) -> tola_typst::SourceBoundary {
        match self.configuration.as_ref() {
            Some(config) => self.resources.source_boundary(config),
            None => crate::sources::base_source_boundary(
                &self.source_root(),
                self.resources.input_scope() == tola_build::InputScope::Pure,
            ),
        }
    }

    pub(super) fn package_locations(&self) -> Option<tola_typst::PackageLocations> {
        self.configuration
            .as_ref()
            .map(|config| self.resources.package_locations(config))
    }

    pub(super) fn source(&self, uri: &Uri) -> Option<Source> {
        if is_untitled(uri) {
            return self.open.source(uri).cloned();
        }
        crate::sources::load(
            uri,
            &self.source_root(),
            self.open.overrides(),
            &self.source_boundary(),
            self.open.view().package_sources(),
            self.open.source(uri),
        )
    }

    fn text_request<T>(&self, uri: &Uri, answer: impl FnOnce(&Source) -> Option<T>) -> Option<T> {
        self.source(uri).and_then(|source| answer(&source))
    }

    /// The style this connection's formatting answers write in.
    fn formatter_style(&self) -> tola_typst_syntax::format::Options {
        crate::formatting::style(
            self.configuration
                .as_deref()
                .map(ResolvedSiteConfig::formatter),
        )
    }

    pub(super) fn formatting(
        &mut self,
        id: RequestId,
        parameters: DocumentFormattingParams,
    ) -> Result<()> {
        let style = self.formatter_style();
        let edits = self.text_request(&parameters.text_document.uri, |source| {
            crate::formatting::edits(source.text(), &style)
        });
        self.reply(Response::new_ok(id, edits))
    }

    /// The edits that format one range of a source, which leaves the rest as the author wrote it.
    pub(super) fn range_formatting(
        &mut self,
        id: RequestId,
        parameters: DocumentRangeFormattingParams,
    ) -> Result<()> {
        let style = self.formatter_style();
        let edits = self.text_request(&parameters.text_document.uri, |source| {
            let lines = source.lines();
            let bytes = crate::position::byte_offset(lines, parameters.range.start).ok()?
                ..crate::position::byte_offset(lines, parameters.range.end).ok()?;
            crate::formatting::range_edits(source.text(), bytes, &style)
        });
        self.reply(Response::new_ok(id, edits))
    }

    pub(super) fn folding(&mut self, id: RequestId, parameters: FoldingRangeParams) -> Result<()> {
        let workspace = self.workspace();
        let line_folding_only = workspace.line_folding_only;
        let ranges = self.text_request(&parameters.text_document.uri, |source| {
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
        let symbols = self.text_request(uri, |source| {
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
        let root = self.source_root();
        let client_root = self.client_root();
        let links = self
            .source(&parameters.text_document.uri)
            .and_then(|source| crate::links::paths(&source, &root, &client_root));
        self.reply(Response::new_ok(id, links))
    }

    pub(super) fn enter(&mut self, id: RequestId, parameters: EnterParams) -> Result<()> {
        let edits = self.text_request(&parameters.uri, |source| {
            crate::on_enter::edits(source, parameters.position)
        });
        self.reply(Response::new_ok(id, EnterReply { edits }))
    }

    fn enqueue_tokens(&mut self, id: RequestId, uri: Uri, kind: TokensKind) -> Result<()> {
        let Some(source) = self.source(&uri) else {
            return self.reply(Response::new_ok(
                id,
                Option::<lsp_types::SemanticTokens>::None,
            ));
        };
        self.enqueue_analysis(id, source, AnalysisTask::Tokens(TokensRead { uri, kind }))
    }

    pub(super) fn commit_tokens(
        &mut self,
        id: RequestId,
        read: TokensRead,
        mut tokens: lsp_types::SemanticTokens,
    ) -> Result<()> {
        // A range read answers for a slice and holds no full-document baseline to delta against.
        let delta = match read.kind {
            TokensKind::Range(_) => {
                self.tokens.entry(read.uri).or_default();
                return self.reply(Response::new_ok(id, tokens));
            }
            TokensKind::Full => None,
            TokensKind::Delta(previous_result_id) => self
                .tokens
                .get(&read.uri)
                .and_then(Option::as_ref)
                .filter(|previous| {
                    previous.result_id.as_deref() == Some(previous_result_id.as_str())
                })
                .map(|previous| crate::tokens::delta(previous, &tokens)),
        };
        self.tokens_serial += 1;
        tokens.result_id = Some(self.tokens_serial.to_string());
        let reply = match delta {
            Some(mut delta) => {
                delta.result_id.clone_from(&tokens.result_id);
                Response::new_ok(id, delta)
            }
            None => Response::new_ok(id, &tokens),
        };
        self.tokens.insert(read.uri, Some(tokens));
        self.reply(reply)
    }

    pub(super) fn tokens_delta(
        &mut self,
        id: RequestId,
        parameters: SemanticTokensDeltaParams,
    ) -> Result<()> {
        self.enqueue_tokens(
            id,
            parameters.text_document.uri,
            TokensKind::Delta(parameters.previous_result_id),
        )
    }

    pub(super) fn tokens(&mut self, id: RequestId, parameters: SemanticTokensParams) -> Result<()> {
        self.enqueue_tokens(id, parameters.text_document.uri, TokensKind::Full)
    }

    pub(super) fn tokens_range(
        &mut self,
        id: RequestId,
        parameters: SemanticTokensRangeParams,
    ) -> Result<()> {
        self.enqueue_tokens(
            id,
            parameters.text_document.uri,
            TokensKind::Range(parameters.range),
        )
    }

    /// The configuration document this workspace resolves.
    ///
    /// It is the file the last completed check resolved, and the file Tola looks for at the
    /// workspace root until one has: an author may name the configuration with `--config` or reach
    /// it through a symlink, so only the resolved path identifies it.
    pub(super) fn configuration_path(&self, root: &Path) -> PathBuf {
        let path = self
            .configuration
            .as_deref()
            .map(ResolvedSiteConfig::config_path)
            .or(self.named_configuration.as_deref())
            .map_or_else(
                || root.join(tola_build::config::loading::CONFIG_FILE_NAME),
                Path::to_path_buf,
            );
        tola_build::filesystem::normalize_existing_prefix(&path)
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
        match self
            .open
            .overrides()
            .iter()
            .find(|(open, _)| **open == normalized)
        {
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
        let configuration = self.configuration_path(root);
        if requested != configuration {
            return None;
        }
        let text = self.document_text(&configuration)?;
        match query {
            SourceQuery::Completion(_) => Some(SourceReply::Completion(CompletionResponse::Array(
                crate::query::config::document(text.as_ref(), position, self.host_sections),
            ))),
            SourceQuery::Hover(_) => Some(SourceReply::Hover(
                crate::query::config::document_hover(text.as_ref(), position, self.host_sections),
            )),
            SourceQuery::CodeActions(params) => {
                let only = params.context.only.as_deref();
                if !crate::code_actions::admits(only, &CodeActionKind::QUICKFIX) {
                    // A fix-all request reads the document's own diagnostics, which this lane does
                    // not answer, so the request continues to the handler that computes them.
                    if crate::code_actions::only_names(only, &CodeActionKind::SOURCE_FIX_ALL) {
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
        let root = self.source_root();
        let uri = &parameters.text_document_position_params.text_document.uri;
        let position = parameters.text_document_position_params.position;
        let Some(source) = self.source(uri) else {
            return self.source_reply(id, SourceReply::PrepareCallHierarchy(None));
        };
        let Ok(cursor) = crate::position::byte_offset(source.lines(), position) else {
            return self.source_reply(id, SourceReply::PrepareCallHierarchy(None));
        };
        let boundary = self.source_boundary();
        let item = crate::call_hierarchy::prepare(
            &root,
            self.package_locations().as_ref(),
            &boundary,
            &source,
            cursor,
            &self.client_root(),
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
        let root = self.source_root();
        let Some(source) = self.source(&parameters.item.uri).filter(|source| {
            parameters
                .item
                .data
                .as_ref()
                .and_then(|data| crate::call_hierarchy::item_file(data, &root))
                .is_some_and(|node| node == source.id())
        }) else {
            return self.source_reply(id, SourceReply::OutgoingCalls(None));
        };
        let boundary = self.source_boundary();
        let calls = crate::call_hierarchy::outgoing(
            &root,
            self.package_locations().as_ref(),
            &boundary,
            &source,
            &self.client_root(),
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
        let uri = &parameters.text_document.uri;
        let synced = self.open.version(uri.as_str());
        match workspace
            .diagnostics
            .pull(uri, parameters.previous_result_id.as_deref(), synced)
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
                response
                    .response_result
                    .as_mut()
                    .expect_err("a rejection carries its error")
                    .data = Some(data);
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
        finish_analysis, messages, open_document, open_draft, ready, ready_at, ready_workspace,
        response,
    };
    use crate::server::tests::load_configuration;
    use lsp_server::{Message, Notification, Request};
    use lsp_types::SemanticTokensFullDeltaResult;
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
        finish_analysis(connection);
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
    fn package_mirrors_stay_outside_site_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Body]").unwrap();
        let configuration = crate::compiler::tests::site_configuration(&root, "");
        let mut compiler = crate::compiler::tests::compiler(&configuration);
        let mirrors = root.join("package-sources");
        let package: Uri = "tola-package:/tola/address/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&package, &root).unwrap();
        let text = crate::identity::embedded_source(id).unwrap();
        let path = mirrors.join("tola/address/0.0.0/lib.typ");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text.as_bytes()).unwrap();
        let uri = crate::uri::from_file_path(&path).unwrap();
        let mut connection = ready_at(Vec::new(), root.clone());
        connection.open =
            crate::sources::OpenSources::new(&root).with_package_sources(Some(Arc::new(mirrors)));
        open_document(&mut connection, &uri, 1, &text);
        assert!(connection.open.overrides().is_empty());
        assert_eq!(connection.source(&uri).unwrap().id(), id);
        connection
            .request(Request::new(
                27.into(),
                request::WorkspaceSymbolRequest::METHOD.into(),
                json!({"query": ""}),
            ))
            .unwrap();
        let job = connection.next_job(Instant::now()).unwrap();
        assert!(matches!(job, SourceJob::Symbols(_)));
        connection.completed(compiler.compile(job)).unwrap();
        let symbols: Vec<lsp_types::WorkspaceSymbol> =
            serde_json::from_value(reply_for(&mut connection, &27.into())).unwrap();
        assert!(
            symbols.iter().all(|symbol| match &symbol.location {
                lsp_types::OneOf::Left(location) => location.uri != uri,
                lsp_types::OneOf::Right(location) => location.uri != uri,
            }),
            "package mirror leaked into workspace symbols: {symbols:?}"
        );
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
        finish_analysis(&mut connection);
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
        finish_analysis(&mut connection);
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
        invalid.line += 1;
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
    fn call_hierarchy_answers_protocol() {
        let site = tempfile::tempdir().unwrap();
        let root = site.path().canonicalize().unwrap();
        std::fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": join-route, output-of
#for source in all-sources() {
  document(output-of(join-route(source.route-segments)))[#include source.file]
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
