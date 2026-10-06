//! One language-server connection's serving lifecycle.
//!
//! A detached reader turns client input into events, two bounded lanes run source checks and
//! name-graph analyses as jobs arrive, and one loop drives the connection until the client or the
//! host ends it.

mod events;
mod lanes;
mod reader;
mod serve;

pub(crate) use lanes::PanickedJob;
pub use serve::{HostSections, ServedWorkspace, serve};

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::io::{BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
    use lsp_server::{Message, Notification, Request, RequestId, Response};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{
        CodeLens, CompletionItem, CompletionResponse, DidChangeTextDocumentParams,
        DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DocumentFormattingParams, DocumentLink, DocumentLinkParams, DocumentSymbolParams,
        DocumentSymbolResponse, FileChangeType, FileEvent, FoldingRange, FoldingRangeParams,
        FormattingOptions, GotoDefinitionResponse, Hover, InitializeParams, InitializeResult,
        Position, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
        TextDocumentPositionParams, TextEdit, Uri, VersionedTextDocumentIdentifier,
        WorkspaceSymbol, WorkspaceSymbolResponse, request as lsp_request,
    };
    use tola_build::BuildResources;
    use tola_build::cancellation::BuildCanceller;
    use tola_build::config::loading::{BuildOverrides, load_site_config};
    use tola_build::diagnostic::Diagnostic;
    use tola_typst::typst::syntax::Source;

    use crate::server::ServedWorkspace;
    use crate::{position, uri};

    pub(super) const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

    const ENTRY_PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": decode-url-path, route, route-to-output, slugify
#for source in all-sources() {
  let declared = if source.meta == none { none } else {
    source.meta.at("permalink", default: none)
  }
  let route = if declared == none {
    route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))
  } else {
    decode-url-path(declared)
  }
  document(route-to-output(route))[#include source.file]
}
"#;

    pub(super) struct SiteDirectory {
        directory: tempfile::TempDir,
    }

    impl SiteDirectory {
        pub(super) fn new() -> Self {
            let site = Self::empty();
            site.write("site.typ", ENTRY_PROGRAM);
            site
        }

        pub(super) fn empty() -> Self {
            let directory = tempfile::tempdir().expect("a temporary site directory");
            let site = Self { directory };
            site.write("tola.toml", "");
            std::fs::create_dir_all(site.path("content")).expect("a content root");
            site
        }

        /// A workspace that holds no site configuration: no `tola.toml` at or above its root.
        pub(super) fn documents() -> Self {
            let directory = tempfile::tempdir().expect("a temporary workspace directory");
            let site = Self { directory };
            std::fs::create_dir_all(site.path("content")).expect("a content root");
            site
        }

        pub(super) fn root(&self) -> &Path {
            self.directory.path()
        }

        pub(super) fn path(&self, relative: &str) -> PathBuf {
            self.root().join(relative)
        }

        pub(super) fn uri(&self, relative: &str) -> Uri {
            uri::from_file_path(&self.path(relative)).expect("a site file is addressable")
        }

        pub(super) fn write(&self, relative: &str, text: &str) {
            let path = self.path(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("a site directory");
            }
            std::fs::write(&path, text).expect("a site file");
        }

        pub(super) fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.path(relative)).expect("a site file")
        }
    }

    pub(super) struct EditorSession {
        root: PathBuf,
        outgoing: std::io::PipeWriter,
        incoming: Receiver<Message>,
        exit: Receiver<Result<()>>,
        /// Messages no earlier wait claimed, in arrival order.
        stashed: VecDeque<Message>,
        versions: BTreeMap<String, i32>,
        next_id: i32,
        canceller: BuildCanceller,
    }

    impl EditorSession {
        pub(super) fn start(site: &SiteDirectory) -> Self {
            Self::start_configured(site, |root, _| load_configuration(root), None)
        }

        /// A session whose site selects its configuration by name, as `--config` does.
        pub(super) fn start_named(site: &SiteDirectory, configuration: PathBuf) -> Self {
            Self::start_configured(
                site,
                |root, _| load_configuration(root),
                Some(configuration),
            )
        }

        pub(super) fn start_with_loader<F>(site: &SiteDirectory, load: F) -> Self
        where
            F: FnMut(&Path, &[(PathBuf, Arc<str>)]) -> Result<ServedWorkspace> + Send + 'static,
        {
            Self::start_configured(site, load, None)
        }

        fn start_configured<F>(
            site: &SiteDirectory,
            load: F,
            named_configuration: Option<PathBuf>,
        ) -> Self
        where
            F: FnMut(&Path, &[(PathBuf, Arc<str>)]) -> Result<ServedWorkspace> + Send + 'static,
        {
            let (server_input, to_server) = std::io::pipe().expect("an in-process pipe");
            let (from_server, server_output) = std::io::pipe().expect("an in-process pipe");
            let canceller = BuildCanceller::new();
            let server_cancellation = canceller.token();
            let (exit, exited) = crossbeam_channel::bounded(1);
            let (messages, incoming) = crossbeam_channel::unbounded();
            thread::Builder::new()
                .name("tola-lsp-test-client".to_owned())
                .spawn(move || read_messages(from_server, &messages))
                .expect("a client reader thread");
            // The handle is dropped: the session ends by protocol, and `shutdown`/`abort` wait on the
            // outcome the thread reports.
            thread::Builder::new()
                .name("tola-lsp-test-server".to_owned())
                .spawn(move || {
                    let outcome = crate::serve(
                        server_input,
                        server_output,
                        BuildResources::new()
                            .with_network_access(tola_build::NetworkAccess::Denied)
                            .without_system_fonts(),
                        server_cancellation,
                        load,
                        |_: &[Diagnostic]| {},
                        named_configuration,
                        &[],
                    );
                    let _ = exit.send(outcome);
                })
                .expect("a server thread");

            let mut client = Self {
                root: site.root().to_path_buf(),
                outgoing: to_server,
                incoming,
                exit: exited,
                stashed: VecDeque::new(),
                versions: BTreeMap::new(),
                next_id: 1,
                canceller,
            };
            client.initialize();
            client
        }

        pub(super) fn open(&mut self, relative: &str, text: &str) -> i32 {
            let uri = uri_string(&self.root, relative);
            self.send(Message::Notification(Notification::new(
                notification::DidOpenTextDocument::METHOD.to_owned(),
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.parse().expect("a document URI"),
                        language_id: "typst".to_owned(),
                        version: 1,
                        text: text.to_owned(),
                    },
                },
            )));
            self.versions.insert(uri, 1);
            1
        }

        pub(super) fn set_text(&mut self, relative: &str, text: &str) -> i32 {
            self.change(
                relative,
                vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_owned(),
                }],
            )
        }

        pub(super) fn type_marked(&mut self, relative: &str, marked: &str) -> Position {
            let (text, cursor) = marked_cursor(marked);
            if self
                .versions
                .contains_key(&uri_string(&self.root, relative))
            {
                self.set_text(relative, &text);
            } else {
                self.open(relative, &text);
            }
            cursor
        }

        pub(super) fn close(&mut self, relative: &str) {
            let uri = uri_string(&self.root, relative);
            self.send(Message::Notification(Notification::new(
                notification::DidCloseTextDocument::METHOD.to_owned(),
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri.parse().expect("a document URI"),
                    },
                },
            )));
            self.versions.remove(&uri);
        }

        pub(super) fn diagnostics(&mut self, relative: &str) -> Vec<lsp_types::Diagnostic> {
            let uri = uri_string(&self.root, relative);
            let version = self.versions.get(&uri).copied();
            self.await_diagnostics(&uri, |_, published| published == version)
                .0
        }

        pub(super) fn diagnostics_matching(
            &mut self,
            relative: &str,
            accept: impl Fn(&[lsp_types::Diagnostic]) -> bool,
        ) -> Vec<lsp_types::Diagnostic> {
            let uri = uri_string(&self.root, relative);
            self.await_diagnostics(&uri, |published, _| accept(published))
                .0
        }

        pub(super) fn complete_marked(
            &mut self,
            relative: &str,
            marked: &str,
        ) -> Vec<CompletionItem> {
            let position = self.type_marked(relative, marked);
            match serde_json::from_value::<Option<CompletionResponse>>(self.query(
                lsp_request::Completion::METHOD,
                relative,
                position,
            ))
            .expect("a completion response")
            {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            }
        }

        pub(super) fn hover_marked(&mut self, relative: &str, marked: &str) -> Option<Hover> {
            let position = self.type_marked(relative, marked);
            serde_json::from_value(self.query(
                lsp_request::HoverRequest::METHOD,
                relative,
                position,
            ))
            .expect("a hover response")
        }

        pub(super) fn definition_marked(
            &mut self,
            relative: &str,
            marked: &str,
        ) -> Option<GotoDefinitionResponse> {
            let position = self.type_marked(relative, marked);
            serde_json::from_value(self.query(
                lsp_request::GotoDefinition::METHOD,
                relative,
                position,
            ))
            .expect("a definition response")
        }

        pub(super) fn package_source(&mut self, package_uri: &str) -> String {
            let result = self.request("tola/source", serde_json::json!({ "uri": package_uri }));
            serde_json::from_value::<crate::protocol::PackageSourceText>(result)
                .expect("a package source response")
                .text
        }

        pub(super) fn formatting(
            &mut self,
            relative: &str,
            options: FormattingOptions,
        ) -> Option<Vec<TextEdit>> {
            let result = self.request(
                lsp_request::Formatting::METHOD,
                serde_json::to_value(DocumentFormattingParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri_string(&self.root, relative)
                            .parse()
                            .expect("a document URI"),
                    },
                    options,
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                })
                .expect("formatting parameters"),
            );
            serde_json::from_value(result).expect("a formatting response")
        }

        pub(super) fn range_formatting(
            &mut self,
            relative: &str,
            range: lsp_types::Range,
            options: FormattingOptions,
        ) -> Option<Vec<TextEdit>> {
            let uri = uri_string(&self.root, relative);
            let reply = self.request(
        lsp_types::request::RangeFormatting::METHOD,
        serde_json::json!({ "textDocument": { "uri": uri }, "range": range, "options": options }),
    );
            serde_json::from_value(reply).expect("a range formatting response")
        }

        pub(super) fn folding(&mut self, relative: &str) -> Option<Vec<FoldingRange>> {
            let result = self.request(
                lsp_request::FoldingRangeRequest::METHOD,
                serde_json::to_value(FoldingRangeParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri_string(&self.root, relative)
                            .parse()
                            .expect("a document URI"),
                    },
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                    partial_result_params: lsp_types::PartialResultParams::default(),
                })
                .expect("folding parameters"),
            );
            serde_json::from_value(result).expect("a folding response")
        }

        pub(super) fn symbols(&mut self, relative: &str) -> Option<DocumentSymbolResponse> {
            let result = self.request(
                lsp_request::DocumentSymbolRequest::METHOD,
                serde_json::to_value(DocumentSymbolParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri_string(&self.root, relative)
                            .parse()
                            .expect("a document URI"),
                    },
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                    partial_result_params: lsp_types::PartialResultParams::default(),
                })
                .expect("symbol parameters"),
            );
            serde_json::from_value(result).expect("a symbol response")
        }

        pub(super) fn routes(&mut self, relative: &str) -> crate::protocol::RouteReply {
            let result = self.request(
                "tola/route",
                serde_json::json!({ "uri": uri_string(&self.root, relative) }),
            );
            serde_json::from_value(result).expect("a route reply")
        }

        pub(super) fn code_actions(
            &mut self,
            relative: &str,
            marked: &str,
            diagnostics: &[lsp_types::Diagnostic],
            only: &[&str],
        ) -> Vec<serde_json::Value> {
            let position = self.type_marked(relative, marked);
            let uri = uri_string(&self.root, relative);
            let reply = self.request(
                lsp_request::CodeActionRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "range": { "start": position, "end": position },
                    "context": { "diagnostics": diagnostics, "only": only },
                }),
            );
            reply.as_array().cloned().unwrap_or_default()
        }

        pub(super) fn code_actions_of_kind(
            &mut self,
            relative: &str,
            marked: &str,
            only: &[&str],
        ) -> Vec<serde_json::Value> {
            self.code_actions(relative, marked, &[], only)
        }

        /// The actions a document answers when the client echoes the diagnostics it shows.
        pub(super) fn code_actions_for_diagnostics(
            &mut self,
            relative: &str,
            marked: &str,
            diagnostics: &[lsp_types::Diagnostic],
        ) -> Vec<serde_json::Value> {
            self.code_actions(relative, marked, diagnostics, &["quickfix"])
        }

        pub(super) fn code_lenses(&mut self, relative: &str) -> Vec<CodeLens> {
            let result = self.request(
                lsp_request::CodeLensRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri_string(&self.root, relative) },
                }),
            );
            serde_json::from_value::<Option<Vec<CodeLens>>>(result)
                .expect("a code lens reply")
                .unwrap_or_default()
        }

        pub(super) fn links(&mut self, relative: &str) -> Option<Vec<DocumentLink>> {
            let result = self.request(
                lsp_request::DocumentLinkRequest::METHOD,
                serde_json::to_value(DocumentLinkParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri_string(&self.root, relative)
                            .parse()
                            .expect("a document URI"),
                    },
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                    partial_result_params: lsp_types::PartialResultParams::default(),
                })
                .expect("link parameters"),
            );
            serde_json::from_value(result).expect("a link response")
        }

        pub(super) fn changed_on_disk(&mut self, relative: &str) {
            let uri = uri_string(&self.root, relative);
            self.send(Message::Notification(Notification::new(
                notification::DidChangeWatchedFiles::METHOD.to_owned(),
                DidChangeWatchedFilesParams {
                    changes: vec![FileEvent {
                        uri: uri.parse().expect("a document URI"),
                        typ: FileChangeType::CHANGED,
                    }],
                },
            )));
        }

        pub(super) fn error_of(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> (i32, String) {
            self.try_request(method, params)
                .expect_err("the request must be rejected")
        }

        pub(super) fn shutdown(mut self) {
            let response = self.request_response("shutdown", serde_json::Value::Null);
            assert!(response.response_result.is_ok(), "{response:?}");
            self.send(Message::Notification(Notification::new(
                notification::Exit::METHOD.to_owned(),
                serde_json::Value::Null,
            )));
            match self.exit.recv_timeout(REPLY_TIMEOUT) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => panic!("the language server ended with an error: {error:#}"),
                Err(_) => panic!("the language server did not end after `exit`"),
            }
        }

        pub(super) fn abort(self) {
            self.canceller.cancel();
            assert!(
                self.exit.recv_timeout(REPLY_TIMEOUT).is_ok(),
                "the language server did not end after its client disappeared"
            );
        }

        fn initialize(&mut self) {
            let result = self.request(
                lsp_request::Initialize::METHOD,
                serde_json::to_value(InitializeParams {
                    process_id: None,
                    workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                        uri: uri::from_file_path(&self.root).expect("a site root URI"),
                        name: "site".to_owned(),
                    }]),
                    capabilities: announcing_client(),
                    ..Default::default()
                })
                .expect("initialize parameters"),
            );
            serde_json::from_value::<InitializeResult>(result).expect("an initialize result");
            self.send(Message::Notification(Notification::new(
                notification::Initialized::METHOD.to_owned(),
                serde_json::json!({}),
            )));
        }

        fn change(&mut self, relative: &str, changes: Vec<TextDocumentContentChangeEvent>) -> i32 {
            let uri = uri_string(&self.root, relative);
            let version = self
                .versions
                .get_mut(&uri)
                .expect("the document is open before it changes");
            *version += 1;
            let version = *version;
            self.send(Message::Notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.to_owned(),
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri.parse().expect("a document URI"),
                        version,
                    },
                    content_changes: changes,
                },
            )));
            version
        }

        pub(super) fn enter_marked(
            &mut self,
            relative: &str,
            marked: &str,
        ) -> Option<Vec<TextEdit>> {
            let position = self.type_marked(relative, marked);
            let uri = uri_string(&self.root, relative);
            let reply = self.request(
                "tola/onEnter",
                serde_json::json!({ "uri": uri, "position": position }),
            );
            serde_json::from_value::<crate::protocol::EnterReply>(reply)
                .expect("an enter reply")
                .edits
        }

        pub(super) fn selection(
            &mut self,
            relative: &str,
            position: Position,
        ) -> serde_json::Value {
            let uri = uri_string(&self.root, relative);
            self.request(
                "textDocument/selectionRange",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "positions": [position],
                }),
            )
        }

        pub(super) fn workspace_symbols(&mut self, query: &str) -> Vec<WorkspaceSymbol> {
            let reply = self.request("workspace/symbol", serde_json::json!({ "query": query }));
            match serde_json::from_value::<WorkspaceSymbolResponse>(reply).expect("a symbol reply")
            {
                WorkspaceSymbolResponse::Nested(symbols) => symbols,
                // Both shapes hold the same fields, and the protocol lets a client read either.
                WorkspaceSymbolResponse::Flat(items) => items
                    .into_iter()
                    .map(|item| WorkspaceSymbol {
                        name: item.name,
                        kind: item.kind,
                        tags: None,
                        container_name: item.container_name,
                        location: lsp_types::OneOf::Left(item.location),
                        data: None,
                    })
                    .collect(),
            }
        }

        fn query(&mut self, method: &str, relative: &str, position: Position) -> serde_json::Value {
            self.request(
                method,
                serde_json::to_value(TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: uri_string(&self.root, relative)
                            .parse()
                            .expect("a document URI"),
                    },
                    position,
                })
                .expect("query parameters"),
            )
        }

        pub(super) fn request(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> serde_json::Value {
            match self.request_response(method, params).response_result {
                Ok(result) => result,
                Err(error) => panic!("{method} failed with {}: {}", error.code, error.message),
            }
        }

        fn try_request(
            &mut self,
            method: &str,
            params: serde_json::Value,
        ) -> std::result::Result<serde_json::Value, (i32, String)> {
            match self.request_response(method, params).response_result {
                Ok(result) => Ok(result),
                Err(error) => Err((error.code, error.message)),
            }
        }

        fn request_response(&mut self, method: &str, params: serde_json::Value) -> Response {
            let id = RequestId::from(self.next_id);
            self.next_id += 1;
            self.send(Message::Request(Request::new(
                id.clone(),
                method.to_owned(),
                params,
            )));
            self.await_message(|message| match message {
                Message::Response(response) if response.id == id => Some(response.clone()),
                _ => None,
            })
        }

        pub(super) fn await_diagnostics(
            &mut self,
            uri: &str,
            accept: impl Fn(&[lsp_types::Diagnostic], Option<i32>) -> bool,
        ) -> (Vec<lsp_types::Diagnostic>, Option<i32>) {
            let uri = uri.to_owned();
            self.await_message(move |message| {
                let Message::Notification(notification) = message else {
                    return None;
                };
                if notification.method != notification::PublishDiagnostics::METHOD {
                    return None;
                }
                let parameters: lsp_types::PublishDiagnosticsParams =
                    serde_json::from_value(notification.params.clone())
                        .expect("published diagnostics");
                if parameters.uri.as_str() != uri
                    || !accept(&parameters.diagnostics, parameters.version)
                {
                    return None;
                }
                Some((parameters.diagnostics, parameters.version))
            })
        }

        fn await_message<T>(&mut self, accept: impl Fn(&Message) -> Option<T>) -> T {
            for index in 0..self.stashed.len() {
                if let Some(claimed) = accept(&self.stashed[index]) {
                    self.stashed.remove(index);
                    return claimed;
                }
            }
            loop {
                let message = match self.incoming.recv_timeout(REPLY_TIMEOUT) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        panic!("the language server did not answer within {REPLY_TIMEOUT:?}")
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        panic!("the language server stopped before answering")
                    }
                };
                match accept(&message) {
                    Some(claimed) => return claimed,
                    None => self.stashed.push_back(message),
                }
            }
        }

        fn send(&mut self, message: Message) {
            message
                .write(&mut self.outgoing)
                .expect("the language server connection is writable");
            self.outgoing
                .flush()
                .expect("the client flushes its output");
        }
    }

    impl Drop for EditorSession {
        fn drop(&mut self) {
            // A test that never said goodbye must still end the server's reader and loop.
            self.canceller.cancel();
        }
    }

    fn read_messages(from_server: std::io::PipeReader, messages: &Sender<Message>) {
        let mut reader = BufReader::new(from_server);
        while let Ok(Some(message)) = Message::read(&mut reader) {
            if messages.send(message).is_err() {
                return;
            }
        }
    }

    /// The site configuration below `root`, as every test's loader resolves it.
    pub(crate) fn load_configuration(root: &Path) -> Result<ServedWorkspace> {
        let config = root.join("tola.toml");
        Ok(ServedWorkspace::Site(Arc::new(
            load_site_config(
                Some(&config),
                tola_typst::PackageLocations::from_absolute_roots(None, None)?,
                &BuildOverrides::default(),
            )
            .context("the site configuration loads")?
            .into_config(),
        )))
    }

    pub(super) fn uri_string(root: &Path, relative: &str) -> String {
        uri::from_file_path(&root.join(relative))
            .expect("a site file is addressable")
            .as_str()
            .to_owned()
    }

    pub(super) fn marked_cursor(marked: &str) -> (String, Position) {
        let offset = marked.find('|').expect("a marked source has a cursor");
        let text = marked.replacen('|', "", 1);
        let source = Source::detached(text.clone());
        let cursor = position::utf16_range(source.lines(), offset..offset)
            .expect("the cursor lies inside the source")
            .start;
        (text, cursor)
    }

    pub(super) fn announcing_client() -> lsp_types::ClientCapabilities {
        serde_json::from_value(serde_json::json!({
            "textDocument": {
                "publishDiagnostics": { "versionSupport": true, "relatedInformation": true, "dataSupport": true },
                "codeLens": {},
                "inlayHint": {},
                "hover": { "contentFormat": ["markdown"] },
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "documentationFormat": ["markdown"]
                    }
                },
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": ["quickfix"] }
                    },
                    "honorsChangeAnnotations": true,
                    "isPreferredSupport": true
                }
            },
            "workspace": {
                "workspaceEdit": {
                    "documentChanges": true,
                    "resourceOperations": ["create", "rename", "delete"],
                    "changeAnnotationSupport": {}
                }
            }
        }))
        .expect("editor capabilities")
    }

    pub(super) fn diagnostic_messages(diagnostics: &[lsp_types::Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect()
    }

    pub(super) fn site_with_document() -> SiteDirectory {
        let site = SiteDirectory::new();
        site.write("content/document.typ", "");
        site
    }
}
