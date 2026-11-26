//! Per-client LSP connection state, tracked sources, pending requests, and worker queues.

mod changes;
mod code_actions;
mod commands;
mod completion;
mod lifecycle;
mod local_requests;
mod progress;
mod refresh;
mod replies;
mod routing;
mod scheduling;
mod source_jobs;
mod status;

use std::collections::BTreeMap;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use lsp_server::{ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{CodeActionOrCommand, NumberOrString, Uri};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_diagnostic::Diagnostic;

use self::scheduling::JobQueue;
use crate::capabilities::ClientFeatures;
use crate::compiler::{AnalysisRequest, SourceInputs, SourceJob};
use crate::diagnostic::ClientDiagnostics;
use crate::protocol::CheckMode;
use crate::server::HostSections;
use crate::sources::OpenSources;
use crate::transport::{InvalidMessage, write_invalid};

/// Clients supporting inlay hints but not code lenses (e.g., Helix) receive page routes as hints.
pub(crate) fn routes_as_hints(caps: &lsp_types::ClientCapabilities) -> bool {
    caps.text_document
        .as_ref()
        .is_some_and(|doc| doc.inlay_hint.is_some() && doc.code_lens.is_none())
}

struct PendingCheck {
    checked_revision: u64,
    queued_at: Instant,
    due: Instant,
    sources: SourceInputs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CompletedCheck {
    revision: u64,
    usable_world: bool,
}

struct SelectionBuild {
    revision: u64,
    serial: u64,
    canceller: BuildCanceller,
}

pub(super) struct Connection<W, D> {
    writer: W,
    record_diagnostics: D,
    shutdown: BuildCanceller,
    resources: tola_build::BuildResources,
    phase: Phase,
    open: OpenSources,
    revision: u64,
    serial: u64,
    checking: BuildCanceller,
    requests: BTreeMap<RequestId, PendingRequest>,
    queries: JobQueue<SourceJob>,
    analyses: JobQueue<AnalysisRequest>,
    check: Option<PendingCheck>,
    selection_build: Option<SelectionBuild>,
    /// Failed reloads retain the last accepted package locations and configuration identity.
    configuration: Option<Arc<tola_build::config::SiteConfig>>,
    named_configuration: Option<PathBuf>,
    running_check: Option<u64>,
    checked: Option<CompletedCheck>,
    read_paths: std::collections::BTreeSet<PathBuf>,
    query_read_paths: std::collections::BTreeSet<PathBuf>,
    progress: Option<Progress>,
    progress_create: Option<PendingCreation>,
    progress_agreed: bool,
    warm: bool,
    identifier_serial: u64,
    settings_pull: Option<RequestId>,
    named_origin: Option<String>,
    default_origin: Option<String>,
    host_sections: HostSections,
    last_work: Option<Instant>,
    announced_panic: bool,
    /// Cached token lists; `None` marks a range reply with no full-document delta baseline.
    tokens: std::collections::HashMap<Uri, Option<lsp_types::SemanticTokens>>,
    tokens_serial: u64,
    semantic_refresh: refresh::SemanticRefresh,
    watchers: changes::Watchers,
}

struct Progress {
    revision: u64,
    token: NumberOrString,
    due: Instant,
    opened: bool,
}

struct PendingCreation {
    id: RequestId,
    token: NumberOrString,
}

impl<W, D> Connection<W, D> {
    pub(super) fn new(
        writer: W,
        record_diagnostics: D,
        shutdown: BuildCanceller,
        resources: tola_build::BuildResources,
        named_configuration: Option<PathBuf>,
        default_origin: Option<String>,
        host_sections: HostSections,
    ) -> Self {
        Self {
            writer,
            record_diagnostics,
            shutdown,
            resources,
            named_configuration,
            phase: Phase::Uninitialized,
            open: OpenSources::default(),
            revision: 0,
            serial: 0,
            checking: BuildCanceller::new(),
            requests: BTreeMap::new(),
            queries: JobQueue::new(),
            analyses: JobQueue::new(),
            check: None,
            selection_build: None,
            running_check: None,
            checked: None,
            read_paths: std::collections::BTreeSet::new(),
            query_read_paths: std::collections::BTreeSet::new(),
            progress: None,
            progress_create: None,
            progress_agreed: true,
            warm: false,
            identifier_serial: 0,
            settings_pull: None,
            named_origin: None,
            default_origin,
            host_sections,
            last_work: None,
            announced_panic: false,
            configuration: None,
            tokens: std::collections::HashMap::new(),
            tokens_serial: 0,
            semantic_refresh: refresh::SemanticRefresh::default(),
            watchers: changes::Watchers::default(),
        }
    }

    pub(super) fn is_shutdown(&self) -> bool {
        matches!(self.phase, Phase::Shutdown)
    }

    pub(super) fn cancel(&self) {
        self.shutdown.cancel();
        self.checking.cancel();
        if let Some(build) = &self.selection_build {
            build.canceller.cancel();
        }
        for req in self.requests.values() {
            req.canceller.cancel();
        }
    }

    fn cancel_selection(&mut self) {
        if let Some(build) = self.selection_build.take() {
            build.canceller.cancel();
            self.queries.retain(
                |job| !matches!(job, SourceJob::Selection(job) if job.serial == build.serial),
            );
        }
    }

    fn source_inputs(&self, root: PathBuf, cancel: BuildCancellation) -> SourceInputs {
        SourceInputs {
            root,
            overrides: Arc::clone(self.open.overrides()),
            source_revision: self.open.revision(),
            cancellation: cancel,
        }
    }

    fn workspace(&self) -> &Workspace {
        let Phase::Ready(ws) = &self.phase else {
            unreachable!("the connection is initialized before it serves anything");
        };
        ws
    }
}

impl<W, D> Drop for Connection<W, D> {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    fn source_root(&self) -> PathBuf {
        let root = self
            .configuration
            .as_ref()
            .map(|cfg| cfg.root())
            .or_else(|| self.named_configuration.as_deref().and_then(Path::parent))
            .unwrap_or_else(|| match &self.phase {
                Phase::Ready(ws) | Phase::Initializing(ws) => &ws.root,
                _ => unreachable!("source roots are selected during initialization"),
            });
        tola_input::normalize_existing_prefix(root)
    }

    pub(super) fn receive(
        &mut self,
        msg: std::result::Result<Message, InvalidMessage>,
    ) -> Result<ControlFlow<()>> {
        match msg {
            Err(invalid) => write_invalid(&mut self.writer, invalid)?,
            Ok(Message::Request(req)) => self.request(req)?,
            Ok(Message::Notification(msg)) => return Ok(self.notification(msg)?),
            Ok(Message::Response(resp)) => self.response(resp)?,
        }
        Ok(ControlFlow::Continue(()))
    }

    fn reply(&mut self, resp: Response) -> Result<()> {
        let msg: Message = resp.into();
        msg.write(&mut self.writer)?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: impl serde::Serialize) -> Result<()> {
        let msg: Message = Notification::new(method.to_owned(), params).into();
        msg.write(&mut self.writer)?;
        Ok(())
    }

    fn send_request(&mut self, method: &str, params: impl serde::Serialize) -> Result<RequestId> {
        let id = RequestId::from(format!("tola-{}", self.next_identifier()));
        let msg: Message = Request::new(id.clone(), method.to_owned(), params).into();
        msg.write(&mut self.writer)?;
        Ok(id)
    }

    fn next_identifier(&mut self) -> u64 {
        self.identifier_serial += 1;
        self.identifier_serial
    }

    fn cancel_pending_work(&mut self, refusal: (ErrorCode, &str)) -> Result<()> {
        self.checking.cancel();
        self.cancel_selection();
        self.cancel_requests(refusal)?;
        self.end_progress()?;
        self.queries.clear();
        self.analyses.clear();
        self.check = None;
        Ok(())
    }
}

struct PendingRequest {
    serial: u64,
    canceller: BuildCanceller,
    /// Open-source revisions stay distinct when a closed URI restarts its document version.
    document_revision: Option<(Uri, u64)>,
    narrowing: Option<Vec<CodeActionOrCommand>>,
    waiting: Option<PendingSelection>,
}

struct PendingSelection {
    revision: u64,
    params: lsp_types::CodeActionParams,
}

enum Phase {
    Uninitialized,
    Initializing(Workspace),
    Ready(Workspace),
    Shutdown,
}

struct Workspace {
    root: PathBuf,
    diagnostics: ClientDiagnostics,
    check_mode: CheckMode,
    source_check_status: bool,
    preview_origin: Option<String>,
    line_folding_only: bool,
    routes_as_hints: bool,
    features: ClientFeatures,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::SourceQuery;
    use lsp_server::Notification;
    use lsp_types::InitializeParams;
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::path::Path;
    use tola_typst::typst::syntax::Source;

    pub(super) fn ready_workspace(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
    ) -> &mut Workspace {
        let Phase::Ready(ws) = &mut conn.phase else {
            unreachable!("a ready connection");
        };
        ws
    }

    pub(crate) fn finish_analysis(conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
        let req = conn.next_analysis().expect("a queued source analysis");
        answer_analysis(conn, req);
    }

    pub(crate) fn answer_analysis(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        req: AnalysisRequest,
    ) {
        conn.completed(crate::compiler::analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            req,
        ))
        .expect("the analysis answered");
    }

    pub(crate) fn dynamic_import_position(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        dir: &Path,
    ) -> (Uri, lsp_types::Position) {
        std::fs::write(dir.join("library.typ"), "#let greet = [hello]\n").unwrap();
        let uri = crate::uri::from_file_path(&dir.join("page.typ")).unwrap();
        let text = "#let base = \"library\"\n#import base + \".typ\": greet\n#greet\n";
        open_document(conn, &uri, 1, text);
        assert!(matches!(
            conn.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let pos = last_position(text, "greet");
        (uri, pos)
    }

    pub(crate) fn last_position(text: &str, needle: &str) -> lsp_types::Position {
        let src = Source::detached(text);
        let byte = text.rfind(needle).unwrap();
        crate::position::utf16_range(src.lines(), byte..byte)
            .unwrap()
            .start
    }

    pub(crate) fn references_request(uri: &Uri, pos: lsp_types::Position) -> Request {
        Request::new(
            8.into(),
            request::References::METHOD.into(),
            json!({
                "textDocument":{"uri":uri.as_str()}, "position":pos, "context":{"includeDeclaration":true}
            }),
        )
    }

    pub(crate) fn ready<W: Write>(writer: W) -> Connection<W, impl Fn(&[Diagnostic])> {
        ready_at(writer, std::env::current_dir().unwrap())
    }

    pub(crate) fn ready_at<W: Write>(
        writer: W,
        root: PathBuf,
    ) -> Connection<W, impl Fn(&[Diagnostic])> {
        let mut conn = Connection::new(
            writer,
            |_: &[Diagnostic]| {},
            BuildCanceller::new(),
            tola_build::BuildResources::default(),
            None,
            None,
            &[],
        );
        conn.open = OpenSources::new(&root);
        conn.phase = Phase::Ready(Workspace {
            diagnostics: ClientDiagnostics::new(root.clone()),
            root,
            check_mode: CheckMode::default(),
            source_check_status: false,
            preview_origin: None,
            routes_as_hints: false,
            line_folding_only: false,
            features: ClientFeatures::default(),
        });
        conn
    }

    pub(crate) fn uninitialized() -> Connection<Vec<u8>, impl Fn(&[Diagnostic])> {
        Connection::new(
            Vec::new(),
            |_: &[Diagnostic]| {},
            BuildCanceller::new(),
            tola_build::BuildResources::default(),
            None,
            None,
            &[],
        )
    }

    pub(crate) fn initialize(conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
        let root = std::env::current_dir().unwrap();
        conn.request(Request::new(
            1.into(),
            request::Initialize::METHOD.into(),
            serde_json::to_value(InitializeParams {
                workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                    uri: crate::uri::from_file_path(&root).unwrap(),
                    name: "site".to_owned(),
                }]),
                ..Default::default()
            })
            .unwrap(),
        ))
        .unwrap();
    }

    pub(crate) fn open_document(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        uri: &Uri,
        version: i32,
        text: &str,
    ) {
        let _ = conn
            .notification(Notification::new(
                notification::DidOpenTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str(),"languageId":"typst","version":version,"text":text}}),
            ))
            .unwrap();
    }

    pub(crate) fn open_draft(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        text: &str,
    ) -> Uri {
        let uri: Uri = "untitled:Draft".parse().unwrap();
        open_document(conn, &uri, 1, text);
        uri
    }

    pub(crate) fn response(conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) -> Response {
        messages(conn)
            .into_iter()
            .find_map(|msg| match msg {
                Message::Response(resp) => Some(resp),
                _ => None,
            })
            .unwrap_or_else(|| panic!("request produced no response"))
    }

    pub(crate) fn rejection(
        conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        id: &RequestId,
    ) -> i32 {
        messages(conn)
            .into_iter()
            .find_map(|msg| match msg {
                Message::Response(resp) if resp.id == *id => {
                    Some(resp.response_result.expect_err("a refusal").code)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no reply for {id:?}"))
    }

    pub(crate) fn hover_query() -> SourceQuery {
        SourceQuery::Hover(lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier::new(
                "file:///site/document.typ".parse().expect("a document URI"),
            ),
            position: lsp_types::Position::new(0, 0),
        })
    }

    pub(crate) fn messages(conn: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) -> Vec<Message> {
        let mut reader = std::io::Cursor::new(std::mem::take(&mut conn.writer));
        let mut msgs = Vec::new();
        while let Some(msg) = Message::read(&mut reader).expect("a framed message") {
            msgs.push(msg);
        }
        msgs
    }
}
