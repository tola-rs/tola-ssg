//! One client's connection: its protocol phase, tracked sources, pending requests, and the two
//! work lanes' queues.
//!
//! This module owns the state every submodule reads and writes and the entry points a host drives
//! the connection through; each submodule answers one part of the protocol.

mod changes;
mod code_actions;
mod commands;
mod completion;
mod lifecycle;
mod local_requests;
mod progress;
mod replies;
mod routing;
mod scheduling;
mod source_jobs;

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{CodeActionOrCommand, NumberOrString, Uri};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::diagnostic::Diagnostic;

use crate::capabilities::ClientFeatures;
use crate::compiler::{AnalysisRequest, SourceInputs, SourceJob, SourceOverrides};
use crate::diagnostic::ClientDiagnostics;
use crate::protocol::{CheckMode, FormatterOptions};
use crate::server::HostSections;
use crate::sources::OpenSources;
use crate::transport::{InvalidMessage, write_invalid};

/// Whether a source's pages reach the author as inlay hints.
///
/// A client that shows code lenses reads each page's address from the lens that opens it. A client
/// that shows hints and no lenses has nothing else to read it from — Helix reads no lenses — so the
/// pages arrive as annotations there instead.
pub(crate) fn routes_as_hints(capabilities: &lsp_types::ClientCapabilities) -> bool {
    capabilities
        .text_document
        .as_ref()
        .is_some_and(|document| document.inlay_hint.is_some() && document.code_lens.is_none())
}

/// A check waiting for dispatch: the revision it answers and the sources it reads, before the
/// selection evidence is frozen with them.
struct PendingCheck {
    checked_revision: u64,
    sources: SourceInputs,
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
    overrides: SourceOverrides,
    checking: BuildCanceller,
    requests: BTreeMap<RequestId, PendingRequest>,
    queries: VecDeque<SourceJob>,
    analyses: VecDeque<AnalysisRequest>,
    check: Option<PendingCheck>,
    /// One build serves the revision's waiting corrections independently of check cancellation.
    selection_build: Option<SelectionBuild>,
    /// When the pending check may run: a typing change sets it after the settle delay, every other
    /// change sets it now.
    check_due: Option<Instant>,
    /// Failed reloads retain the last accepted package locations and configuration identity.
    configuration: Option<Arc<tola_build::config::ResolvedSiteConfig>>,
    /// The configuration an author named with `--config`, kept before a check resolves one, so the
    /// editor's own document answers while the site does not compile.
    named_configuration: Option<PathBuf>,
    /// The source revision whose check the compiler lane is running now.
    running_check: Option<u64>,
    /// The revision and outcome of the last completed source check: whether the worlds it resolved
    /// are usable. The check lane computes the flag (`CheckedSources::compiled`) — the Bundle in
    /// site mode, the documents the check covered in document mode — and `CheckProgress::Failed`
    /// reads `false` as "no usable world".
    checked: Option<(u64, bool)>,
    /// The disk paths the last completed check read: the evidence deciding whether a changed file
    /// can change what this connection answers.
    read_paths: std::collections::BTreeSet<PathBuf>,
    /// The check whose progress the client is being shown, while it runs.
    progress: Option<Progress>,
    /// The report whose creation the client has not answered, and when it was asked: a creation
    /// reply may outlive its check, so it is matched by its own id and token, and one the client
    /// leaves unanswered past its bound is given up rather than waited on forever.
    progress_create: Option<PendingCreation>,
    /// Whether this client created a progress report without an error; a client that refused one
    /// is never sent another.
    progress_agreed: bool,

    /// Whether a check has already finished on this connection: the first check is the one the
    /// author is waiting for, so it is shown at once, while a later one is shown only once it runs
    /// long.
    warm: bool,
    /// The identifier the next request this connection sends has: request identifiers are
    /// scoped to their sender, so this sequence never meets the client's.
    request_serial: u64,
    /// The `workspace/configuration` request whose answer has this client's settings.
    settings_pull: Option<RequestId>,
    /// The origin a client named for the development server, which outranks the site's own
    /// generated state.
    named_origin: Option<String>,
    /// The origin a command line named, which stands in for one the site's generated state holds.
    default_origin: Option<String>,
    /// The configuration sections this host declares, which a `tola.toml` answer reads its keys
    /// from.
    host_sections: HostSections,
    /// The moment this connection last had work, which an idle interval is measured from.
    active: Instant,
    /// Whether the compiled site is waiting to be let go, which the next free compiler slot runs.
    release: bool,
    /// Whether the compiler lane was already asked to let go of the compiled site, so a connection
    /// that stays idle asks once rather than on every turn.
    released: bool,
    /// Whether the author was already told the site's own configuration could not be read.
    announced_configuration_failure: bool,
    /// Whether the author was already told a check's own work failed, so one failing run of checks
    /// announces itself once rather than on every edit.
    announced_panic: bool,
    /// The tokens each document last answered with, for the next delta.
    tokens: std::collections::HashMap<Uri, lsp_types::SemanticTokens>,
    /// The identifier the next token result has, which a delta request echoes.
    tokens_serial: u64,
    /// The site's own Typst files as this connection's direct answers last read them: a source
    /// change drops them, and the compiler and analysis lanes each keep their own.
    disk: crate::analysis::DiskSources,
    /// The name graphs this connection's own answers last built, keyed by the revision they belong
    /// to: a source change starts a revision the cache cannot answer for.
    graphs: crate::analysis::GraphCache,
}

/// One open work-done report.
struct Progress {
    /// The revision the report belongs to: a check superseded by a newer revision ends this report
    /// without ending the one the newer check opened.
    revision: u64,
    token: NumberOrString,
    /// The moment the report may open; a check that finishes before it never opens one.
    due: Instant,
    /// Whether the client has been shown the report, so ending it is all that is left.
    opened: bool,
}

/// One report whose creation the client has not answered yet.
struct PendingCreation {
    /// The id the creation request was sent under, which its answer has.
    id: RequestId,
    /// The report's token, which its answer must name for that report to open.
    token: NumberOrString,
    /// When the request was sent, so a client that never answers one is not waited on forever.
    sent: Instant,
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
            overrides: Arc::default(),
            checking: BuildCanceller::new(),
            requests: BTreeMap::new(),
            queries: VecDeque::new(),
            analyses: VecDeque::new(),
            check: None,
            selection_build: None,
            check_due: None,
            running_check: None,
            checked: None,
            read_paths: std::collections::BTreeSet::new(),
            progress: None,
            progress_create: None,
            progress_agreed: true,
            warm: false,
            request_serial: 0,
            settings_pull: None,
            named_origin: None,
            default_origin,
            host_sections,
            active: Instant::now(),
            release: false,
            released: false,
            announced_configuration_failure: false,
            announced_panic: false,
            configuration: None,
            tokens: std::collections::HashMap::new(),
            tokens_serial: 0,
            disk: crate::analysis::DiskSources::default(),
            graphs: crate::analysis::GraphCache::default(),
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
        for request in self.requests.values() {
            request.canceller.cancel();
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

    /// The inputs one source job reads: this connection's overrides at the revision its tracked
    /// sources hold.
    fn source_inputs(&self, root: PathBuf, cancellation: BuildCancellation) -> SourceInputs {
        SourceInputs {
            root,
            overrides: Arc::clone(&self.overrides),
            source_revision: self.open.revision(),
            cancellation,
        }
    }

    /// The workspace this connection serves.
    ///
    /// Every request, notification, and lane completion this type receives runs after `initialize`
    /// answered, so a connection that never reached `Phase::Ready` runs none of them.
    fn workspace(&self) -> &Workspace {
        let Phase::Ready(workspace) = &self.phase else {
            unreachable!("the connection is initialized before it serves anything");
        };
        workspace
    }
}

impl<W, D> Drop for Connection<W, D> {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn receive(
        &mut self,
        message: std::result::Result<Message, InvalidMessage>,
    ) -> Result<ControlFlow<()>> {
        match message {
            Err(invalid) => write_invalid(&mut self.writer, invalid)?,
            Ok(Message::Request(request)) => self.request(request)?,
            Ok(Message::Notification(message)) => return self.notification(message),
            Ok(Message::Response(response)) => self.response(response)?,
        }
        Ok(ControlFlow::Continue(()))
    }

    /// Write one response.
    ///
    /// A client that stopped reading blocks this write, and with it the connection: no cancellation
    /// reaches a write already in flight, so the host's token is observed between messages and the
    /// connection stalls until the client reads again or the process ends.
    fn reply(&mut self, response: Response) -> Result<()> {
        let message: Message = response.into();
        message.write(&mut self.writer)?;
        Ok(())
    }

    /// Write one notification to the client.
    fn notify(&mut self, method: &str, params: impl serde::Serialize) -> Result<()> {
        let message: Message = Notification::new(method.to_owned(), params).into();
        message.write(&mut self.writer)?;
        Ok(())
    }

    /// Send one request to the client, under this connection's own identifier sequence.
    fn send_request(
        &mut self,
        method: &str,
        parameters: impl serde::Serialize,
    ) -> Result<RequestId> {
        self.request_serial += 1;
        let id = RequestId::from(format!("tola-{}", self.request_serial));
        let message: Message = Request::new(id.clone(), method.to_owned(), parameters).into();
        message.write(&mut self.writer)?;
        Ok(id)
    }
}

struct PendingRequest {
    serial: u64,
    canceller: BuildCanceller,
    /// The fixes a code action request already earned, held until the compiler lane adds the
    /// narrowing actions the checked world justifies.
    narrowing: Option<Vec<CodeActionOrCommand>>,
    /// The code action this request stands in, held while the revision's selection index is built
    /// on the compiler lane.
    waiting: Option<PendingSelection>,
}

/// One code action a revision has no accepted selection index for.
struct PendingSelection {
    /// The revision whose index the answer reads.
    revision: u64,
    /// The request as the client sent it, which is answered unchanged once that index exists.
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
    package_sources: Option<Arc<PathBuf>>,
    /// How the client's settings shape a formatted source.
    formatter: FormatterOptions,
    /// When a source change reruns the site's check, which the client states for this site.
    check_mode: CheckMode,
    source_check_status: bool,

    /// Where the client's launch configuration says the development server serves the site, as
    /// `http://host:port` with no path; the site's own mount path is read from its configuration.
    preview_origin: Option<String>,
    /// Whether the client folds complete lines only, which asks for no character columns.
    line_folding_only: bool,
    /// Whether a source's pages reach the author as inlay hints, which a client showing no code
    /// lens reads.
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
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
    ) -> &mut Workspace {
        let Phase::Ready(workspace) = &mut connection.phase else {
            unreachable!("a ready connection");
        };
        workspace
    }

    pub(crate) fn dynamic_import_position(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        directory: &Path,
    ) -> (Uri, lsp_types::Position) {
        std::fs::write(directory.join("library.typ"), "#let greet = [hello]\n").unwrap();
        let uri = crate::uri::from_file_path(&directory.join("page.typ")).unwrap();
        let text = "#let base = \"library\"\n#import base + \".typ\": greet\n#greet\n";
        open_document(connection, &uri, 1, text);
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let source = Source::detached(text);
        let byte = text.rfind("greet").unwrap();
        let position = crate::position::utf16_range(source.lines(), byte..byte)
            .unwrap()
            .start;
        (uri, position)
    }

    pub(crate) fn references_request(uri: &Uri, position: lsp_types::Position) -> Request {
        Request::new(
            8.into(),
            request::References::METHOD.into(),
            json!({
                "textDocument":{"uri":uri.as_str()}, "position":position, "context":{"includeDeclaration":true}
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
        let mut connection = Connection::new(
            writer,
            |_: &[Diagnostic]| {},
            BuildCanceller::new(),
            tola_build::BuildResources::default(),
            None,
            // No launch origin: these tests never send the handshake that names one.
            None,
            &[],
        );
        connection.open = OpenSources::new(&root);
        connection.phase = Phase::Ready(Workspace {
            diagnostics: ClientDiagnostics::new(root.clone()),
            root,
            package_sources: None,
            formatter: FormatterOptions::default(),
            check_mode: CheckMode::default(),
            source_check_status: false,
            preview_origin: None,
            routes_as_hints: false,
            line_folding_only: false,
            features: ClientFeatures::default(),
        });
        connection
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

    pub(crate) fn initialize(connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
        let root = std::env::current_dir().unwrap();
        connection
            .request(Request::new(
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

    /// One `didOpen` of `uri` at `version`, holding `text`.
    pub(crate) fn open_document(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        uri: &Uri,
        version: i32,
        text: &str,
    ) {
        let _ = connection
            .notification(Notification::new(
                notification::DidOpenTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str(),"languageId":"typst","version":version,"text":text}}),
            ))
            .unwrap();
    }

    pub(crate) fn open_draft(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        text: &str,
    ) -> Uri {
        let uri: Uri = "untitled:Draft".parse().unwrap();
        open_document(connection, &uri, 1, text);
        uri
    }

    pub(crate) fn response(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
    ) -> Response {
        messages(connection)
            .into_iter()
            .find_map(|message| match message {
                Message::Response(response) => Some(response),
                _ => None,
            })
            .unwrap_or_else(|| panic!("request produced no response"))
    }

    /// The error code this connection replied to `id` with.
    pub(crate) fn rejection(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
        id: &RequestId,
    ) -> i32 {
        messages(connection)
            .into_iter()
            .find_map(|message| match message {
                Message::Response(response) if response.id == *id => {
                    Some(response.response_result.expect_err("a refusal").code)
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

    /// Every message this connection has written, drained so the next read starts empty.
    pub(crate) fn messages(
        connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>,
    ) -> Vec<Message> {
        let mut reader = std::io::Cursor::new(std::mem::take(&mut connection.writer));
        let mut messages = Vec::new();
        while let Some(message) = Message::read(&mut reader).expect("a framed message") {
            messages.push(message);
        }
        messages
    }
}
