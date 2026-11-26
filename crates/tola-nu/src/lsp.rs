//! Restart the official Nu stdio server when its prepared module inputs change.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_textdocument::FullTextDocument;
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, LogMessageParams, MessageType, Position,
};
use serde_json::{Value, json};

use crate::{Runtime, SourceOverlay};

const POLL: Duration = Duration::from_millis(250);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(2);
const INITIALIZE_ATTEMPTS: u32 = 3;

enum Event {
    Client(Message),
    ClientClosed,
    Worker(u64, Message),
    WorkerClosed(u64, String),
}

struct Worker {
    generation: u64,
    child: Child,
    input: ChildStdin,
    runtime: Runtime,
    initialize_id: RequestId,
    ready: bool,
    _snapshot: tempfile::NamedTempFile,
}

impl Worker {
    fn spawn(
        executable: &Path,
        runtime: Runtime,
        builtin_dev: bool,
        generation: u64,
        events: Sender<Event>,
    ) -> Result<Self> {
        let ctx = runtime.invocation();
        let mut snapshot =
            tempfile::NamedTempFile::new().context("create Nu LSP buffer snapshot")?;
        serde_json::to_writer(snapshot.as_file_mut(), runtime.overlays())?;
        snapshot.as_file_mut().sync_all()?;
        let mut cmd = Command::new(executable);
        cmd.args(["nu", "--lsp-worker"])
            .arg("--lsp-snapshot")
            .arg(snapshot.path())
            .current_dir(&ctx.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if builtin_dev {
            cmd.arg("--builtin-dev");
        }
        if let Some(cfg) = runtime.site_config() {
            cmd.arg("--config").arg(cfg);
        }
        for (name, val) in &ctx.environment {
            if let Some(val) = val {
                cmd.env(name, val);
            } else {
                cmd.env_remove(name);
            }
        }
        let mut child = cmd.spawn().context("start Nu language-server worker")?;
        let input = child
            .stdin
            .take()
            .context("Nu worker stdin is unavailable")?;
        let out = child
            .stdout
            .take()
            .context("Nu worker stdout is unavailable")?;
        std::thread::spawn(move || {
            let mut out = BufReader::new(out);
            let reason = loop {
                match Message::read(&mut out) {
                    Ok(Some(msg)) => {
                        if events.send(Event::Worker(generation, msg)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => break "Nu language-server worker closed".into(),
                    Err(err) => {
                        break format!("Nu language-server worker output failed: {err}");
                    }
                }
            };
            let _ = events.send(Event::WorkerClosed(generation, reason));
        });
        Ok(Self {
            generation,
            child,
            input,
            runtime,
            initialize_id: RequestId::from(format!("tola-nu-initialize-{generation}")),
            ready: false,
            _snapshot: snapshot,
        })
    }

    fn send(&mut self, msg: &Message) -> Result<()> {
        msg.write(&mut self.input)
            .context("write Nu worker request")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The supervisor's copy of one open document. It is replayed into every new worker, so its text
/// and version must stay exactly what the client last sent.
struct Document {
    uri: String,
    text: FullTextDocument,
}

impl Document {
    fn open(&self) -> Message {
        Message::Notification(Notification::new(
            "textDocument/didOpen".into(),
            json!({"textDocument":{
                "uri": &self.uri,
                "languageId": self.text.language_id(),
                "version": self.text.version(),
                "text": self.text.get_content(None),
            }}),
        ))
    }

    fn change(&mut self, params: DidChangeTextDocumentParams) -> std::result::Result<(), String> {
        let ver = params.text_document.version;
        if ver <= self.text.version() {
            return Err(format!("out-of-order document version {ver}"));
        }
        let mut next = FullTextDocument::new(
            self.text.language_id().to_owned(),
            ver,
            self.text.get_content(None).to_owned(),
        );
        for change in &params.content_changes {
            if let Some(range) = &change.range {
                check_position(&next, range.start)?;
                check_position(&next, range.end)?;
                if next.offset_at(range.start) > next.offset_at(range.end) {
                    return Err("reversed document change range".into());
                }
            }
            next.update(std::slice::from_ref(change), ver);
        }
        // An empty change list still advances the version the worker will read on replay.
        next.update(&[], ver);
        self.text = next;
        Ok(())
    }
}

fn check_position(doc: &FullTextDocument, pos: Position) -> std::result::Result<(), String> {
    if pos.line >= doc.line_count() {
        return Err(format!(
            "document position {}:{} is outside the snapshot",
            pos.line, pos.character
        ));
    }
    Ok(())
}

struct Supervisor<F> {
    executable: PathBuf,
    builtin_dev: bool,
    reload: F,
    events: Sender<Event>,
    worker: Option<Worker>,
    generation: u64,
    initialize: Option<Request>,
    initialized: Option<Value>,
    initialize_replied: bool,
    initialize_attempts: u32,
    initialize_failed: bool,
    retry_at: Instant,
    retry_delay: Duration,
    workspace: PathBuf,
    documents: BTreeMap<String, Document>,
    pending: HashSet<RequestId>,
    server_requests: HashMap<RequestId, RequestId>,
    next_server_request: u64,
    diagnostic_uris: HashSet<String>,
    last_error: Option<String>,
    selected_config: ConfigSelection,
    replayed_generation: Option<u64>,
    shutdown: bool,
}

/// The nearest `tola.toml` the supervisor last prepared against. `Unknown` forces the polling
/// loop to restart once, because a configuration file may have appeared or disappeared while the
/// modules were being prepared.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ConfigSelection {
    Unknown,
    Selected(Option<PathBuf>),
}

impl<F: FnMut(&Path, &[SourceOverlay]) -> Result<Runtime>> Supervisor<F> {
    fn write(msg: Message) -> Result<()> {
        msg.write(&mut io::stdout().lock())
            .context("write Nu LSP response")
    }

    fn error(&mut self, msg: String) -> Result<()> {
        if self.last_error.as_ref() != Some(&msg) {
            Self::write(Message::Notification(Notification::new(
                "window/logMessage".into(),
                LogMessageParams {
                    typ: MessageType::ERROR,
                    message: msg.clone(),
                },
            )))?;
            self.last_error = Some(msg);
        }
        Ok(())
    }

    fn reject(id: RequestId, code: ErrorCode, msg: impl Into<String>) -> Result<()> {
        Self::write(Message::Response(Response::new_err(
            id,
            code as i32,
            msg.into(),
        )))
    }

    fn reject_pending(&mut self, code: ErrorCode, msg: &str) -> Result<()> {
        for id in self.pending.drain().collect::<Vec<_>>() {
            Self::reject(id, code, msg)?;
        }
        Ok(())
    }

    fn init_id(&self) -> Result<RequestId> {
        Ok(self
            .initialize
            .as_ref()
            .context("missing initialize request")?
            .id
            .clone())
    }

    fn invalidate(&mut self, reason: Option<String>) -> Result<()> {
        self.worker.take();
        self.initialize_attempts = 0;
        self.initialize_failed = false;
        self.retry_delay = RETRY_MIN;
        self.retry_at = Instant::now();
        self.server_requests.clear();
        self.reject_pending(
            ErrorCode::ContentModified,
            "Nu module environment changed; retry the request",
        )?;
        for uri in std::mem::take(&mut self.diagnostic_uris) {
            Self::write(Message::Notification(Notification::new(
                "textDocument/publishDiagnostics".into(),
                json!({"uri": uri, "diagnostics": []}),
            )))?;
        }
        if let Some(reason) = reason {
            self.error(reason)
        } else {
            Ok(())
        }
    }

    fn start(&mut self) -> Result<()> {
        let Some(init) = self.initialize.as_ref() else {
            return Ok(());
        };
        let init_params = init.params.clone();
        let overlays = self
            .documents
            .values()
            .filter_map(|doc| {
                Some(SourceOverlay {
                    path: uri_to_path(&doc.uri)?,
                    text: doc.text.get_content(None).to_owned(),
                    version: i64::from(doc.text.version()),
                })
            })
            .collect::<Vec<_>>();
        let cfg_before = nearest_config(&self.workspace);
        let runtime = (self.reload)(&self.workspace, &overlays)?;
        let cfg_after = nearest_config(&self.workspace);
        self.selected_config = if cfg_before == cfg_after {
            ConfigSelection::Selected(cfg_after)
        } else {
            ConfigSelection::Unknown
        };
        self.generation += 1;
        let mut worker = Worker::spawn(
            &self.executable,
            runtime,
            self.builtin_dev,
            self.generation,
            self.events.clone(),
        )?;
        worker.send(&Message::Request(Request::new(
            worker.initialize_id.clone(),
            "initialize".into(),
            init_params,
        )))?;
        self.worker = Some(worker);
        self.initialize_attempts = 0;
        self.retry_delay = RETRY_MIN;
        Ok(())
    }

    fn initialize_failure(&mut self, err: anyhow::Error) -> Result<()> {
        self.initialize_attempts += 1;
        self.retry_delay = (self.retry_delay * 2).min(RETRY_MAX);
        self.retry_at = Instant::now() + self.retry_delay;
        let msg = format!("cannot prepare Nu modules: {err:#}");
        if self.initialize_attempts >= INITIALIZE_ATTEMPTS && !self.initialize_replied {
            let id = self.init_id()?;
            self.initialize_replied = true;
            self.initialize_failed = true;
            Self::reject(
                id,
                ErrorCode::InternalError,
                format!("{msg}; fix the site configuration and restart the language server"),
            )?;
        } else {
            self.error(msg)?;
        }
        Ok(())
    }

    /// A worker that stops before answering initialize counts as a failed attempt, so a crashing
    /// worker cannot make the supervisor retry forever without ever answering the client.
    fn worker_lost(&mut self, reason: String) -> Result<()> {
        self.worker.take();
        if self.initialize_replied {
            self.invalidate(Some(reason))
        } else {
            self.initialize_failure(anyhow!("Nu language-server worker stopped: {reason}"))
        }
    }

    fn replay(&mut self) -> Result<()> {
        let Some(initialized) = self.initialized.clone() else {
            return Ok(());
        };
        let Some(worker) = self.worker.as_mut() else {
            return Ok(());
        };
        let generation = worker.generation;
        if self.replayed_generation == Some(generation) {
            return Ok(());
        }
        worker.send(&Message::Notification(Notification::new(
            "initialized".into(),
            initialized,
        )))?;
        for doc in self.documents.values() {
            worker.send(&doc.open())?;
        }
        worker.ready = true;
        self.replayed_generation = Some(generation);
        Ok(())
    }

    fn forward(&mut self, msg: Message) -> Result<()> {
        let Some(worker) = &mut self.worker else {
            return Ok(());
        };
        if let Err(err) = worker.send(&msg) {
            self.invalidate(Some(format!("Nu language-server worker stopped: {err:#}")))?;
        }
        Ok(())
    }

    fn worker_ready(&self) -> bool {
        self.worker.as_ref().is_some_and(|w| w.ready)
    }

    fn parse_params<T: serde::de::DeserializeOwned>(
        &mut self,
        notif: &Notification,
        kind: &str,
    ) -> Result<Option<T>> {
        match serde_json::from_value(notif.params.clone()) {
            Ok(params) => Ok(Some(params)),
            Err(err) => {
                self.error(format!("ignored an invalid Nu document {kind}: {err}"))?;
                Ok(None)
            }
        }
    }

    fn forward_or_restart(&mut self, uri: &str, notif: Notification) -> Result<()> {
        if self.restarts_for(uri) {
            return self.invalidate(None);
        }
        if self.worker_ready() {
            self.forward(Message::Notification(notif))?;
        }
        Ok(())
    }

    fn restarts_for(&self, uri: &str) -> bool {
        uri_to_path(uri)
            .and_then(|path| crate::modules::source_path(&path).ok())
            .is_some_and(|path| {
                self.worker
                    .as_ref()
                    .is_some_and(|w| w.runtime.source_paths.contains(&path))
            })
    }

    fn did_open(&mut self, notif: Notification) -> Result<()> {
        let Some(params) = self.parse_params::<DidOpenTextDocumentParams>(&notif, "open")? else {
            return Ok(());
        };
        let td = params.text_document;
        let doc = Document {
            uri: td.uri.as_str().to_owned(),
            text: FullTextDocument::new(td.language_id, td.version, td.text),
        };
        let uri = doc.uri.clone();
        self.documents.insert(uri.clone(), doc);
        self.forward_or_restart(&uri, notif)
    }

    fn did_change(&mut self, notif: Notification) -> Result<()> {
        let Some(params) = self.parse_params::<DidChangeTextDocumentParams>(&notif, "change")?
        else {
            return Ok(());
        };
        let uri = params.text_document.uri.as_str().to_owned();
        let Some(doc) = self.documents.get_mut(&uri) else {
            return self
                .error("ignored a Nu document change for a document that is not open".into());
        };
        if let Err(msg) = doc.change(params) {
            return self.error(format!("ignored a Nu document change: {msg}"));
        }
        self.forward_or_restart(&uri, notif)
    }

    fn did_close(&mut self, notif: Notification) -> Result<()> {
        let Some(params) = self.parse_params::<DidCloseTextDocumentParams>(&notif, "close")? else {
            return Ok(());
        };
        let uri = params.text_document.uri.as_str().to_owned();
        self.documents.remove(&uri);
        self.diagnostic_uris.remove(&uri);
        self.forward_or_restart(&uri, notif)
    }

    fn notification(&mut self, notif: Notification) -> Result<()> {
        match notif.method.as_str() {
            "initialized" => {
                self.initialized = Some(notif.params);
                self.replay()?;
            }
            "textDocument/didOpen" => self.did_open(notif)?,
            "textDocument/didChange" => self.did_change(notif)?,
            "textDocument/didClose" => self.did_close(notif)?,
            _ if self.worker_ready() => self.forward(Message::Notification(notif))?,
            _ => {}
        }
        Ok(())
    }

    fn client(&mut self, msg: Message) -> Result<bool> {
        match msg {
            Message::Request(req) if req.method == "initialize" => {
                if self.initialize.is_some() {
                    Self::reject(
                        req.id,
                        ErrorCode::InvalidRequest,
                        "Nu language server is already initialized",
                    )?;
                    return Ok(true);
                }
                match workspace(&req.params) {
                    Ok(ws) => {
                        self.workspace = ws;
                        self.initialize = Some(req);
                        if let Err(err) = self.start() {
                            self.initialize_failure(err)?;
                        }
                    }
                    Err(msg) => {
                        Self::reject(req.id, ErrorCode::InvalidParams, msg)?;
                    }
                }
            }
            Message::Request(req) if req.method == "shutdown" => {
                self.shutdown = true;
                self.worker.take();
                self.reject_pending(
                    ErrorCode::InvalidRequest,
                    "Nu language server is shutting down",
                )?;
                Self::write(Message::Response(Response::new_ok(req.id, Value::Null)))?;
            }
            Message::Request(req) => {
                if self.shutdown {
                    Self::reject(
                        req.id,
                        ErrorCode::InvalidRequest,
                        "Nu language server is shutting down",
                    )?;
                } else if self.worker_ready() {
                    self.pending.insert(req.id.clone());
                    self.forward(Message::Request(req))?;
                } else {
                    let code = if self.initialize_replied {
                        ErrorCode::ContentModified
                    } else {
                        ErrorCode::ServerNotInitialized
                    };
                    Self::reject(
                        req.id,
                        code,
                        "Nu module environment is not ready; retry the request",
                    )?;
                }
            }
            Message::Notification(notif) if notif.method == "exit" => {
                self.worker.take();
                return Ok(false);
            }
            Message::Notification(notif) => self.notification(notif)?,
            Message::Response(mut resp) => {
                if let Some(orig) = self.server_requests.remove(&resp.id) {
                    resp.id = orig;
                    self.forward(Message::Response(resp))?;
                }
            }
        }
        Ok(true)
    }

    fn worker(&mut self, generation: u64, msg: Message) -> Result<()> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        if worker.generation != generation {
            return Ok(());
        }
        let initialize_id = worker.initialize_id.clone();
        match msg {
            Message::Response(mut resp) if resp.id == initialize_id => {
                if let Err(err) = &resp.response_result {
                    let msg = err.message.clone();
                    self.worker.take();
                    return self.initialize_failure(anyhow!(
                        "Nu language server rejected initialization: {msg}"
                    ));
                }
                self.last_error = None;
                if !self.initialize_replied {
                    resp.id = self.init_id()?;
                    Self::write(Message::Response(resp))?;
                    self.initialize_replied = true;
                }
                self.replay()?;
            }
            Message::Response(resp) => {
                if self.pending.remove(&resp.id) {
                    Self::write(Message::Response(resp))?;
                }
            }
            Message::Request(mut req) => {
                let id = RequestId::from(format!(
                    "tola-nu-{generation}-request-{}",
                    self.next_server_request
                ));
                self.next_server_request += 1;
                self.server_requests.insert(id.clone(), req.id);
                req.id = id;
                Self::write(Message::Request(req))?;
            }
            Message::Notification(notif) => {
                if notif.method == "textDocument/publishDiagnostics"
                    && let Some(uri) = notif.params["uri"].as_str()
                {
                    self.diagnostic_uris.insert(uri.into());
                }
                Self::write(Message::Notification(notif))?;
            }
        }
        Ok(())
    }
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

fn nearest_config(workspace: &Path) -> Option<PathBuf> {
    for dir in workspace.ancestors() {
        let candidate = dir.join("tola.toml");
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Some(candidate),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Some(candidate),
        }
    }
    None
}

fn workspace(params: &Value) -> std::result::Result<PathBuf, String> {
    let params: InitializeParams = serde_json::from_value(params.clone())
        .map_err(|err| format!("invalid initialize parameters: {err}"))?;
    let uri = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| folder.uri.clone());
    if let Some(uri) = uri {
        let url = url::Url::parse(uri.as_str())
            .map_err(|_| "Nu workspace must be a file URI".to_owned())?;
        return url
            .to_file_path()
            .map_err(|()| "Nu workspace must be a local file URI".to_owned());
    }
    std::env::current_dir().map_err(|err| format!("cannot determine Nu workspace: {err}"))
}

/// Supervise fresh official Nu workers, rebuilding selected module bindings and replaying buffers.
pub fn serve_lsp<F>(executable: &Path, builtin_dev: bool, reload: F) -> i32
where
    F: FnMut(&Path, &[SourceOverlay]) -> Result<Runtime>,
{
    match serve(executable, builtin_dev, reload) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            1
        }
    }
}

fn serve<F: FnMut(&Path, &[SourceOverlay]) -> Result<Runtime>>(
    executable: &Path,
    builtin_dev: bool,
    reload: F,
) -> Result<i32> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let interrupt = Arc::clone(&cancelled);
    ctrlc::set_handler(move || interrupt.store(true, Ordering::Relaxed))?;
    let (tx, rx): (Sender<Event>, Receiver<Event>) = crossbeam_channel::unbounded();
    let input = tx.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        while let Ok(Some(msg)) = Message::read(&mut reader) {
            if input.send(Event::Client(msg)).is_err() {
                return;
            }
        }
        let _ = input.send(Event::ClientClosed);
    });
    let mut server = Supervisor {
        executable: executable.into(),
        builtin_dev,
        reload,
        events: tx,
        worker: None,
        generation: 0,
        initialize: None,
        initialized: None,
        initialize_replied: false,
        initialize_attempts: 0,
        initialize_failed: false,
        retry_at: Instant::now(),
        retry_delay: RETRY_MIN,
        workspace: std::env::current_dir()?,
        documents: BTreeMap::new(),
        pending: HashSet::new(),
        server_requests: HashMap::new(),
        next_server_request: 0,
        diagnostic_uris: HashSet::new(),
        last_error: None,
        selected_config: ConfigSelection::Unknown,
        replayed_generation: None,
        shutdown: false,
    };
    let mut checked = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(130);
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Client(msg)) => {
                if !server.client(msg)? {
                    return Ok(if server.shutdown { 0 } else { 1 });
                }
            }
            Ok(Event::ClientClosed) => return Ok(0),
            Ok(Event::Worker(generation, msg)) => {
                if let Err(err) = server.worker(generation, msg) {
                    server.worker_lost(format!("Cannot restore Nu worker session: {err:#}"))?;
                }
            }
            Ok(Event::WorkerClosed(generation, reason)) => {
                if server
                    .worker
                    .as_ref()
                    .is_some_and(|w| w.generation == generation)
                {
                    server.worker_lost(reason)?;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
        }
        if checked.elapsed() >= POLL {
            checked = Instant::now();
            let cfg = nearest_config(&server.workspace);
            let stale = server.worker.as_ref().is_some_and(|w| {
                w.runtime.ensure_fresh().is_err()
                    || server.selected_config != ConfigSelection::Selected(cfg)
            });
            if stale {
                server.invalidate(None)?;
            }
        }
        if server.worker.is_none()
            && server.initialize.is_some()
            && !server.shutdown
            && !server.initialize_failed
            && Instant::now() >= server.retry_at
            && let Err(err) = server.start()
        {
            server.initialize_failure(err)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(version: i32, start: (u32, u32), end: (u32, u32), text: &str) -> Value {
        json!({
            "textDocument": {"uri": "file:///catalog.nu", "version": version},
            "contentChanges": [{
                "range": {
                    "start": {"line": start.0, "character": start.1},
                    "end": {"line": end.0, "character": end.1}
                },
                "text": text
            }]
        })
    }

    fn doc() -> Document {
        Document {
            uri: "file:///catalog.nu".into(),
            text: FullTextDocument::new(
                "nu".into(),
                1,
                "let x = '😀字'
$x"
                .into(),
            ),
        }
    }

    #[test]
    fn incremental_unicode_sync() {
        let mut doc = doc();
        let params = serde_json::from_value(change(2, (0, 9), (0, 11), "λ")).unwrap();
        doc.change(params).unwrap();
        assert_eq!(
            doc.text.get_content(None),
            "let x = 'λ字'
$x"
        );
        assert_eq!(doc.text.version(), 2);
        let Message::Notification(open) = doc.open() else {
            panic!("expected didOpen")
        };
        assert_eq!(open.params["textDocument"]["version"], 2);
        assert_eq!(
            open.params["textDocument"]["text"],
            doc.text.get_content(None)
        );
    }

    #[test]
    fn reject_invalid_range_edit() {
        let mut doc = doc();
        let reversed = serde_json::from_value(change(2, (1, 1), (0, 0), "x")).unwrap();
        assert!(doc.change(reversed).is_err());
        let stale = serde_json::from_value(change(1, (0, 0), (0, 1), "x")).unwrap();
        assert!(doc.change(stale).is_err());
        assert_eq!(
            doc.text.get_content(None),
            "let x = '😀字'
$x"
        );
        assert_eq!(doc.text.version(), 1);
    }

    #[test]
    fn nearest_ancestor_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("nested/workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let outer = dir.path().join("tola.toml");
        let inner = dir.path().join("nested/tola.toml");
        std::fs::write(&outer, "[modules]").unwrap();
        assert_eq!(nearest_config(&ws), Some(outer));
        std::fs::write(&inner, "[modules]").unwrap();
        assert_eq!(nearest_config(&ws), Some(inner.clone()));
        std::fs::remove_file(inner).unwrap();
        assert_eq!(nearest_config(&ws), Some(dir.path().join("tola.toml")));
        std::fs::remove_file(dir.path().join("tola.toml")).unwrap();
        assert_eq!(nearest_config(&ws), None);
    }

    #[test]
    fn workspace_first_folder() {
        let params = json!({
            "processId": null,
            "capabilities": {},
            "workspaceFolders": [{"uri": "file:///folder", "name": "folder"}]
        });
        assert!(workspace(&params).unwrap().ends_with("folder"));
        let invalid = json!({"processId": null, "capabilities": {}});
        assert!(
            workspace(&invalid)
                .unwrap()
                .ends_with(std::env::current_dir().unwrap())
        );
    }
}
