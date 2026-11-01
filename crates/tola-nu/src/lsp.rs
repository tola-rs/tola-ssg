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
        generation: u64,
        events: Sender<Event>,
    ) -> Result<Self> {
        let context = runtime.invocation();
        let mut snapshot =
            tempfile::NamedTempFile::new().context("create Nu LSP buffer snapshot")?;
        serde_json::to_writer(snapshot.as_file_mut(), runtime.overlays())?;
        snapshot.as_file_mut().sync_all()?;
        let mut command = Command::new(executable);
        command
            .args(["nu", "--lsp-worker"])
            .arg("--lsp-snapshot")
            .arg(snapshot.path())
            .current_dir(&context.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (name, value) in &context.environment {
            if let Some(value) = value {
                command.env(name, value);
            } else {
                command.env_remove(name);
            }
        }
        let mut child = command.spawn().context("start Nu language-server worker")?;
        let input = child
            .stdin
            .take()
            .context("Nu worker stdin is unavailable")?;
        let output = child
            .stdout
            .take()
            .context("Nu worker stdout is unavailable")?;
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            let reason = loop {
                match Message::read(&mut output) {
                    Ok(Some(message)) => {
                        if events.send(Event::Worker(generation, message)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => break "Nu language-server worker closed".into(),
                    Err(error) => {
                        break format!("Nu language-server worker output failed: {error}");
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

    fn send(&mut self, message: &Message) -> Result<()> {
        message
            .write(&mut self.input)
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
        let version = params.text_document.version;
        if version <= self.text.version() {
            return Err(format!("out-of-order document version {version}"));
        }
        let mut next = FullTextDocument::new(
            self.text.language_id().to_owned(),
            version,
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
            next.update(std::slice::from_ref(change), version);
        }
        // An empty change list still advances the version the worker will read on replay.
        next.update(&[], version);
        self.text = next;
        Ok(())
    }
}

fn check_position(
    document: &FullTextDocument,
    position: Position,
) -> std::result::Result<(), String> {
    if position.line >= document.line_count() {
        return Err(format!(
            "document position {}:{} is outside the snapshot",
            position.line, position.character
        ));
    }
    Ok(())
}

struct Supervisor<F> {
    executable: PathBuf,
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
    fn write(message: Message) -> Result<()> {
        message
            .write(&mut io::stdout().lock())
            .context("write Nu LSP response")
    }

    fn error(&mut self, message: String) -> Result<()> {
        if self.last_error.as_ref() != Some(&message) {
            Self::write(Message::Notification(Notification::new(
                "window/logMessage".into(),
                LogMessageParams {
                    typ: MessageType::ERROR,
                    message: message.clone(),
                },
            )))?;
            self.last_error = Some(message);
        }
        Ok(())
    }

    fn reject(id: RequestId, code: ErrorCode, message: impl Into<String>) -> Result<()> {
        Self::write(Message::Response(Response::new_err(
            id,
            code as i32,
            message.into(),
        )))
    }

    fn invalidate(&mut self, reason: Option<String>) -> Result<()> {
        self.worker.take();
        self.initialize_attempts = 0;
        self.initialize_failed = false;
        self.retry_delay = RETRY_MIN;
        self.retry_at = Instant::now();
        self.server_requests.clear();
        for id in self.pending.drain().collect::<Vec<_>>() {
            Self::reject(
                id,
                ErrorCode::ContentModified,
                "Nu module environment changed; retry the request",
            )?;
        }
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
        let Some(initialize) = self.initialize.as_ref() else {
            return Ok(());
        };
        let initialize_id = initialize.id.clone();
        let initialize_params = initialize.params.clone();
        let overlays = self
            .documents
            .values()
            .filter_map(|document| {
                let path = url::Url::parse(&document.uri).ok()?.to_file_path().ok()?;
                Some(SourceOverlay {
                    path,
                    text: document.text.get_content(None).to_owned(),
                    version: i64::from(document.text.version()),
                })
            })
            .collect::<Vec<_>>();
        let config_before = nearest_config(&self.workspace);
        let runtime = (self.reload)(&self.workspace, &overlays)?;
        let config_after = nearest_config(&self.workspace);
        self.selected_config = if config_before == config_after {
            ConfigSelection::Selected(config_after)
        } else {
            ConfigSelection::Unknown
        };
        self.generation += 1;
        let mut worker = Worker::spawn(
            &self.executable,
            runtime,
            self.generation,
            self.events.clone(),
        )?;
        worker.send(&Message::Request(Request::new(
            initialize_id,
            "initialize".into(),
            initialize_params,
        )))?;
        self.worker = Some(worker);
        self.initialize_attempts = 0;
        self.retry_delay = RETRY_MIN;
        Ok(())
    }

    fn initialize_failure(&mut self, error: anyhow::Error) -> Result<()> {
        self.initialize_attempts += 1;
        self.retry_delay = (self.retry_delay * 2).min(RETRY_MAX);
        self.retry_at = Instant::now() + self.retry_delay;
        let message = format!("cannot prepare Nu modules: {error:#}");
        if self.initialize_attempts >= INITIALIZE_ATTEMPTS && !self.initialize_replied {
            let id = self
                .initialize
                .as_ref()
                .context("missing initialize request")?
                .id
                .clone();
            self.initialize_replied = true;
            self.initialize_failed = true;
            Self::reject(
                id,
                ErrorCode::InternalError,
                format!("{message}; fix the site configuration and restart the language server"),
            )?;
        } else {
            self.error(message)?;
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
        for document in self.documents.values() {
            worker.send(&document.open())?;
        }
        worker.ready = true;
        self.replayed_generation = Some(generation);
        Ok(())
    }

    fn forward(&mut self, message: Message) -> Result<()> {
        let Some(worker) = &mut self.worker else {
            return Ok(());
        };
        if let Err(error) = worker.send(&message) {
            self.invalidate(Some(format!(
                "Nu language-server worker stopped: {error:#}"
            )))?;
        }
        Ok(())
    }

    fn restarts_for(&self, uri: &str) -> bool {
        url::Url::parse(uri)
            .ok()
            .and_then(|uri| uri.to_file_path().ok())
            .and_then(|path| crate::modules::source_path(&path).ok())
            .is_some_and(|path| {
                self.worker
                    .as_ref()
                    .is_some_and(|worker| worker.runtime.source_paths.contains(&path))
            })
    }

    fn did_open(&mut self, notification: Notification) -> Result<()> {
        let params: DidOpenTextDocumentParams =
            match serde_json::from_value(notification.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return self.error(format!("ignored an invalid Nu document open: {error}"));
                }
            };
        let text = params.text_document;
        let document = Document {
            uri: text.uri.as_str().to_owned(),
            text: FullTextDocument::new(text.language_id, text.version, text.text),
        };
        let uri = document.uri.clone();
        self.documents.insert(uri.clone(), document);
        if self.restarts_for(&uri) {
            return self.invalidate(None);
        }
        if self.worker.as_ref().is_some_and(|worker| worker.ready) {
            self.forward(Message::Notification(notification))?;
        }
        Ok(())
    }

    fn did_change(&mut self, notification: Notification) -> Result<()> {
        let params: DidChangeTextDocumentParams =
            match serde_json::from_value(notification.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return self.error(format!("ignored an invalid Nu document change: {error}"));
                }
            };
        let uri = params.text_document.uri.as_str().to_owned();
        let Some(document) = self.documents.get_mut(&uri) else {
            return self
                .error("ignored a Nu document change for a document that is not open".into());
        };
        if let Err(message) = document.change(params) {
            return self.error(format!("ignored a Nu document change: {message}"));
        }
        if self.restarts_for(&uri) {
            return self.invalidate(None);
        }
        if self.worker.as_ref().is_some_and(|worker| worker.ready) {
            self.forward(Message::Notification(notification))?;
        }
        Ok(())
    }

    fn did_close(&mut self, notification: Notification) -> Result<()> {
        let params: DidCloseTextDocumentParams =
            match serde_json::from_value(notification.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return self.error(format!("ignored an invalid Nu document close: {error}"));
                }
            };
        let uri = params.text_document.uri.as_str().to_owned();
        self.documents.remove(&uri);
        self.diagnostic_uris.remove(&uri);
        if self.restarts_for(&uri) {
            return self.invalidate(None);
        }
        if self.worker.as_ref().is_some_and(|worker| worker.ready) {
            self.forward(Message::Notification(notification))?;
        }
        Ok(())
    }

    fn notification(&mut self, notification: Notification) -> Result<()> {
        match notification.method.as_str() {
            "initialized" => {
                self.initialized = Some(notification.params);
                self.replay()?;
            }
            "textDocument/didOpen" => self.did_open(notification)?,
            "textDocument/didChange" => self.did_change(notification)?,
            "textDocument/didClose" => self.did_close(notification)?,
            _ => {
                if self.worker.as_ref().is_some_and(|worker| worker.ready) {
                    self.forward(Message::Notification(notification))?;
                }
            }
        }
        Ok(())
    }

    fn client(&mut self, message: Message) -> Result<bool> {
        match message {
            Message::Request(request) if request.method == "initialize" => {
                if self.initialize.is_some() {
                    Self::reject(
                        request.id,
                        ErrorCode::InvalidRequest,
                        "Nu language server is already initialized",
                    )?;
                    return Ok(true);
                }
                match workspace(&request.params) {
                    Ok(workspace) => {
                        self.workspace = workspace;
                        self.initialize = Some(request);
                        if let Err(error) = self.start() {
                            self.initialize_failure(error)?;
                        }
                    }
                    Err(message) => {
                        Self::reject(request.id, ErrorCode::InvalidParams, message)?;
                    }
                }
            }
            Message::Request(request) if request.method == "shutdown" => {
                self.shutdown = true;
                self.worker.take();
                for id in self.pending.drain().collect::<Vec<_>>() {
                    Self::reject(
                        id,
                        ErrorCode::InvalidRequest,
                        "Nu language server is shutting down",
                    )?;
                }
                Self::write(Message::Response(Response::new_ok(request.id, Value::Null)))?;
            }
            Message::Request(request) => {
                if self.shutdown {
                    Self::reject(
                        request.id,
                        ErrorCode::InvalidRequest,
                        "Nu language server is shutting down",
                    )?;
                } else if self.worker.as_ref().is_some_and(|worker| worker.ready) {
                    self.pending.insert(request.id.clone());
                    self.forward(Message::Request(request))?;
                } else {
                    let code = if self.initialize_replied {
                        ErrorCode::ContentModified
                    } else {
                        ErrorCode::ServerNotInitialized
                    };
                    Self::reject(
                        request.id,
                        code,
                        "Nu module environment is not ready; retry the request",
                    )?;
                }
            }
            Message::Notification(notification) if notification.method == "exit" => {
                self.worker.take();
                return Ok(false);
            }
            Message::Notification(notification) => self.notification(notification)?,
            Message::Response(mut response) => {
                if let Some(original) = self.server_requests.remove(&response.id) {
                    response.id = original;
                    self.forward(Message::Response(response))?;
                }
            }
        }
        Ok(true)
    }

    fn worker(&mut self, generation: u64, message: Message) -> Result<()> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        if worker.generation != generation {
            return Ok(());
        }
        let initialize_id = worker.initialize_id.clone();
        match message {
            Message::Response(response) if response.id == initialize_id => {
                if let Err(error) = &response.response_result {
                    let message = error.message.clone();
                    self.worker.take();
                    return self.initialize_failure(anyhow!(
                        "Nu language server rejected initialization: {message}"
                    ));
                }
                self.last_error = None;
                if !self.initialize_replied {
                    let mut response = response;
                    response.id = self
                        .initialize
                        .as_ref()
                        .context("missing initialize request")?
                        .id
                        .clone();
                    Self::write(Message::Response(response))?;
                    self.initialize_replied = true;
                }
                self.replay()?;
            }
            Message::Response(response) => {
                if self.pending.remove(&response.id) {
                    Self::write(Message::Response(response))?;
                }
            }
            Message::Request(mut request) => {
                let id = RequestId::from(format!(
                    "tola-nu-{generation}-request-{}",
                    self.next_server_request
                ));
                self.next_server_request += 1;
                self.server_requests.insert(id.clone(), request.id);
                request.id = id;
                Self::write(Message::Request(request))?;
            }
            Message::Notification(notification) => {
                if notification.method == "textDocument/publishDiagnostics"
                    && let Some(uri) = notification.params["uri"].as_str()
                {
                    self.diagnostic_uris.insert(uri.into());
                }
                Self::write(Message::Notification(notification))?;
            }
        }
        Ok(())
    }
}

fn nearest_config(workspace: &Path) -> Option<PathBuf> {
    for directory in workspace.ancestors() {
        let candidate = directory.join("tola.toml");
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Some(candidate),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Some(candidate),
        }
    }
    None
}

fn workspace(params: &Value) -> std::result::Result<PathBuf, String> {
    let params: InitializeParams = serde_json::from_value(params.clone())
        .map_err(|error| format!("invalid initialize parameters: {error}"))?;
    #[allow(deprecated)]
    let uri = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| folder.uri.clone())
        .or(params.root_uri);
    if let Some(uri) = uri {
        let url = url::Url::parse(uri.as_str())
            .map_err(|_| "Nu workspace must be a file URI".to_owned())?;
        return url
            .to_file_path()
            .map_err(|()| "Nu workspace must be a local file URI".to_owned());
    }
    #[allow(deprecated)]
    if let Some(path) = params.root_path {
        return Ok(path.into());
    }
    std::env::current_dir().map_err(|error| format!("cannot determine Nu workspace: {error}"))
}

/// Supervise fresh official Nu workers, rebuilding selected module bindings and replaying buffers.
pub fn serve_lsp<F>(executable: &Path, reload: F) -> i32
where
    F: FnMut(&Path, &[SourceOverlay]) -> Result<Runtime>,
{
    match serve(executable, reload) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

fn serve<F: FnMut(&Path, &[SourceOverlay]) -> Result<Runtime>>(
    executable: &Path,
    reload: F,
) -> Result<i32> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let interrupt = Arc::clone(&cancelled);
    ctrlc::set_handler(move || interrupt.store(true, Ordering::Relaxed))?;
    let (send, receive): (Sender<Event>, Receiver<Event>) = crossbeam_channel::unbounded();
    let input = send.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        while let Ok(Some(message)) = Message::read(&mut reader) {
            if input.send(Event::Client(message)).is_err() {
                return;
            }
        }
        let _ = input.send(Event::ClientClosed);
    });
    let mut server = Supervisor {
        executable: executable.into(),
        reload,
        events: send,
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
        match receive.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Client(message)) => {
                if !server.client(message)? {
                    return Ok(if server.shutdown { 0 } else { 1 });
                }
            }
            Ok(Event::ClientClosed) => return Ok(0),
            Ok(Event::Worker(generation, message)) => {
                if let Err(error) = server.worker(generation, message) {
                    server.worker_lost(format!("Cannot restore Nu worker session: {error:#}"))?;
                }
            }
            Ok(Event::WorkerClosed(generation, reason)) => {
                if server
                    .worker
                    .as_ref()
                    .is_some_and(|worker| worker.generation == generation)
                {
                    server.worker_lost(reason)?;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
        }
        if checked.elapsed() >= POLL {
            checked = Instant::now();
            let config = nearest_config(&server.workspace);
            let stale = server.worker.as_ref().is_some_and(|worker| {
                worker.runtime.ensure_fresh().is_err()
                    || server.selected_config != ConfigSelection::Selected(config)
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
            && let Err(error) = server.start()
        {
            server.initialize_failure(error)?;
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

    fn document() -> Document {
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
    fn document_change_replays_the_latest_unicode_text() {
        let mut document = document();
        let params = serde_json::from_value(change(2, (0, 9), (0, 11), "λ")).unwrap();
        document.change(params).unwrap();
        assert_eq!(
            document.text.get_content(None),
            "let x = 'λ字'
$x"
        );
        assert_eq!(document.text.version(), 2);
        let Message::Notification(open) = document.open() else {
            panic!("expected didOpen")
        };
        assert_eq!(open.params["textDocument"]["version"], 2);
        assert_eq!(
            open.params["textDocument"]["text"],
            document.text.get_content(None)
        );
    }

    #[test]
    fn rejected_change_keeps_the_replayed_text_intact() {
        let mut document = document();
        let reversed = serde_json::from_value(change(2, (1, 1), (0, 0), "x")).unwrap();
        assert!(document.change(reversed).is_err());
        let stale = serde_json::from_value(change(1, (0, 0), (0, 1), "x")).unwrap();
        assert!(document.change(stale).is_err());
        assert_eq!(
            document.text.get_content(None),
            "let x = '😀字'
$x"
        );
        assert_eq!(document.text.version(), 1);
    }

    #[test]
    fn nearest_config_selects_the_closest_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("nested/workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let outer = directory.path().join("tola.toml");
        let inner = directory.path().join("nested/tola.toml");
        std::fs::write(&outer, "[modules]").unwrap();
        assert_eq!(nearest_config(&workspace), Some(outer));
        std::fs::write(&inner, "[modules]").unwrap();
        assert_eq!(nearest_config(&workspace), Some(inner.clone()));
        std::fs::remove_file(inner).unwrap();
        assert_eq!(
            nearest_config(&workspace),
            Some(directory.path().join("tola.toml"))
        );
        std::fs::remove_file(directory.path().join("tola.toml")).unwrap();
        assert_eq!(nearest_config(&workspace), None);
    }

    #[test]
    fn initialize_workspace_prefers_the_first_folder() {
        let params = json!({
            "processId": null,
            "capabilities": {},
            "rootUri": "file:///root",
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
