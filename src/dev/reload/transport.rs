//! WebSocket transport for development-server updates.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures_util::{SinkExt, Stream, StreamExt, stream::FuturesUnordered};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::StreamMap;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::http::{HeaderMap, StatusCode};
use tokio_tungstenite::tungstenite::protocol::frame::{
    Frame,
    coding::{Data, OpCode},
};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};

use crate::dev::reload::message::{DiagnosticCounts, HotReloadMessage};
use crate::dev::site::CurrentSite;
use tola_build::diagnostic::Diagnostic;
use tola_build::output::PageAvailability;
use tola_build::output::manifest::{RevisionDiff, RevisionId};
use tola_build::site::SiteRevision;

const MAX_RELOAD_CLIENTS: usize = 64;
const MAX_PENDING_HANDSHAKES: usize = 8;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);
const MAX_CLIENT_MESSAGE_BYTES: usize = 16 * 1024;
const CLIENT_READ_BUFFER_BYTES: usize = 4 * 1024;
const SERVER_INITIAL_WRITE_BUFFER_BYTES: usize = 4 * 1024;
const SERVER_FRAME_BYTES: usize = 16 * 1024;
const SERVER_WRITE_BUFFER_BYTES: usize = 32 * 1024;
const SESSION_ID_BYTES: usize = 32;

type ClientStream = WebSocketStream<tokio::net::TcpStream>;
type ReloadClients<S = tokio::net::TcpStream> = StreamMap<SocketAddr, ReloadClient<S>>;

#[derive(Clone)]
pub(crate) struct ReloadEndpoint {
    port: u16,
    session_token: std::sync::Arc<str>,
    generation: std::sync::Arc<str>,
}

impl ReloadEndpoint {
    pub(crate) fn new(
        port: u16,
        session_token: impl Into<std::sync::Arc<str>>,
        generation: impl Into<std::sync::Arc<str>>,
    ) -> Self {
        Self {
            port,
            session_token: session_token.into(),
            generation: generation.into(),
        }
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn session_token(&self) -> &str {
        &self.session_token
    }

    /// Public restart identity, generated independently of the authentication token.
    pub(crate) fn generation(&self) -> &str {
        &self.generation
    }
}

enum RevisionChange {
    Unchanged,
    Changed(RevisionDiff),
    Resync(RevisionId),
}

#[derive(Default)]
struct PendingUpdates {
    revision: Option<RevisionChange>,
    /// The newest build round's counts, replacing the retained ones.
    counts: Option<DiagnosticCounts>,
    /// The newest failed after-publish round, replacing the retained failure.
    consumer: Option<ConsumerFailure>,
    /// The newest guard's rebuild state, replacing the retained one.
    rebuilding: Option<bool>,
    shutdown: bool,
}

impl PendingUpdates {
    fn revision_replaced(&mut self, diff: RevisionDiff, counts: DiagnosticCounts) {
        let next = RevisionChange::from_diff(diff);
        self.revision = Some(match (self.revision.take(), next) {
            (None, next) | (Some(RevisionChange::Unchanged), next) => next,
            (Some(previous @ RevisionChange::Changed(_)), RevisionChange::Unchanged)
            | (Some(previous @ RevisionChange::Resync(_)), RevisionChange::Unchanged) => previous,
            (Some(RevisionChange::Changed(previous)), RevisionChange::Changed(next)) => {
                assert_eq!(
                    previous.to(),
                    next.from(),
                    "queued revision diffs must be contiguous"
                );
                RevisionChange::Resync(next.to().clone())
            }
            (Some(RevisionChange::Resync(previous)), RevisionChange::Changed(next)) => {
                assert_eq!(
                    &previous,
                    next.from(),
                    "queued revision diffs must be contiguous"
                );
                RevisionChange::Resync(next.to().clone())
            }
            (Some(RevisionChange::Changed(_)), RevisionChange::Resync(_))
            | (Some(RevisionChange::Resync(_)), RevisionChange::Resync(_)) => {
                unreachable!("new revision changes are never pre-coalesced")
            }
        });
        self.counts = Some(counts);
    }

    fn take(&mut self) -> Self {
        Self {
            revision: self.revision.take(),
            counts: self.counts.take(),
            consumer: self.consumer.take(),
            rebuilding: self.rebuilding.take(),
            shutdown: self.shutdown,
        }
    }
}

/// The failure one after-publish round reported for the revision it consumed.
struct ConsumerFailure {
    /// The installed revision the round consumed; weak, because a superseded site must not be
    /// kept alive by a failure about it.
    revision: Weak<SiteRevision>,
    counts: DiagnosticCounts,
}

/// What a browser indicator counts: the newest build's diagnostics plus the newest
/// after-publish failure, which counts only while its revision stays installed.
#[derive(Default)]
struct IndicatorCounts {
    build: DiagnosticCounts,
    consumer: Option<ConsumerFailure>,
}

impl IndicatorCounts {
    fn build_replaced(&mut self, counts: DiagnosticCounts) {
        self.build = counts;
    }

    fn consumer_failed(&mut self, failure: ConsumerFailure) {
        self.consumer = Some(failure);
    }

    /// The counts to show for the installed `revision`; a replaced site's failure no longer
    /// counts.
    fn for_revision(&self, revision: Option<&Arc<SiteRevision>>) -> DiagnosticCounts {
        let mut counts = self.build;
        if let Some(failure) = &self.consumer
            && revision
                .is_some_and(|revision| Weak::ptr_eq(&failure.revision, &Arc::downgrade(revision)))
        {
            counts.merge(failure.counts);
        }
        counts
    }
}

#[derive(Default)]
struct ReloadUpdateMailbox {
    pending: Mutex<PendingUpdates>,
    available: tokio::sync::Notify,
}

/// Reports an after-publish round's failure without replacing the build's counts.
#[derive(Clone)]
pub(crate) struct ReloadDiagnostics(Arc<ReloadUpdateMailbox>);

impl ReloadDiagnostics {
    /// Report one round's failure for the revision it consumed.
    pub(crate) fn after_publish_failed(
        &self,
        revision: &Arc<SiteRevision>,
        diagnostic: &Diagnostic,
    ) {
        let mut counts = DiagnosticCounts::default();
        counts.count(diagnostic);
        self.0.consumer_failed(ConsumerFailure {
            revision: Arc::downgrade(revision),
            counts,
        });
    }
}

impl ReloadUpdateMailbox {
    fn publish(&self, update: impl FnOnce(&mut PendingUpdates)) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut pending);
        drop(pending);
        self.available.notify_one();
    }

    fn first_revision_ready(&self, revision: RevisionId, counts: DiagnosticCounts) {
        self.publish(|pending| {
            pending.revision = Some(RevisionChange::Resync(revision));
            pending.counts = Some(counts);
        });
    }

    fn revision_replaced(&self, diff: RevisionDiff, counts: DiagnosticCounts) {
        self.publish(|pending| pending.revision_replaced(diff, counts));
    }

    fn counts_replaced(&self, counts: DiagnosticCounts) {
        self.publish(|pending| pending.counts = Some(counts));
    }

    fn consumer_failed(&self, failure: ConsumerFailure) {
        self.publish(|pending| pending.consumer = Some(failure));
    }

    fn begin_rebuild(&self) {
        self.publish(|pending| pending.rebuilding = Some(true));
    }

    fn finish_rebuild(&self) {
        self.publish(|pending| pending.rebuilding = Some(false));
    }

    fn request_shutdown(&self) {
        self.publish(|pending| pending.shutdown = true);
    }

    fn take(&self) -> PendingUpdates {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl RevisionChange {
    fn from_diff(diff: RevisionDiff) -> Self {
        if diff.is_unchanged() {
            Self::Unchanged
        } else {
            Self::Changed(diff)
        }
    }
}

/// Controls the task serving the reload listener and clients.
pub(crate) struct ReloadTransport {
    endpoint: ReloadEndpoint,
    updates: Arc<ReloadUpdateMailbox>,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[must_use = "the guard must remain alive for the complete rebuild"]
pub(crate) struct RebuildStatusGuard {
    updates: Arc<ReloadUpdateMailbox>,
}

impl Drop for RebuildStatusGuard {
    fn drop(&mut self) {
        self.updates.finish_rebuild();
    }
}

impl ReloadTransport {
    pub(in crate::dev) fn start(
        interface: IpAddr,
        base_port: u16,
        sites: CurrentSite,
    ) -> Result<Self> {
        let (listener, port) = bind_listener(interface, base_port)?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let session_token = Arc::<str>::from(new_session_id()?);
        let endpoint = ReloadEndpoint::new(port, Arc::clone(&session_token), new_session_id()?);
        let updates = Arc::new(ReloadUpdateMailbox::default());
        let task_updates = Arc::clone(&updates);
        let task = tokio::spawn(run(listener, task_updates, sites, session_token));

        Ok(Self {
            endpoint,
            updates,
            task: Some(task),
        })
    }

    pub(crate) fn endpoint(&self) -> ReloadEndpoint {
        self.endpoint.clone()
    }

    pub(crate) fn diagnostics_sender(&self) -> ReloadDiagnostics {
        ReloadDiagnostics(Arc::clone(&self.updates))
    }

    pub(crate) fn first_revision_ready(&self, revision: RevisionId, diagnostics: &[Diagnostic]) {
        self.updates
            .first_revision_ready(revision, DiagnosticCounts::of(diagnostics));
    }

    pub(crate) fn revision_replaced(&self, diff: RevisionDiff, diagnostics: &[Diagnostic]) {
        self.updates
            .revision_replaced(diff, DiagnosticCounts::of(diagnostics));
    }

    pub(crate) fn replace_diagnostics(&self, diagnostics: &[Diagnostic]) {
        self.updates
            .counts_replaced(DiagnosticCounts::of(diagnostics));
    }

    pub(crate) fn begin_rebuild(&self) -> RebuildStatusGuard {
        self.updates.begin_rebuild();
        RebuildStatusGuard {
            updates: Arc::clone(&self.updates),
        }
    }

    pub(crate) async fn shutdown(mut self) -> Result<()> {
        self.updates.request_shutdown();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|_| anyhow!("the browser reload server stopped; restart `tola dev`"))
    }
}

impl Drop for ReloadTransport {
    fn drop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        self.updates.request_shutdown();
        task.abort();
    }
}

async fn run(
    listener: tokio::net::TcpListener,
    updates: Arc<ReloadUpdateMailbox>,
    sites: CurrentSite,
    session_token: Arc<str>,
) {
    let mut clients = ReloadClients::new();
    let mut handshakes = tokio::task::JoinSet::new();
    let mut indicator = IndicatorCounts::default();
    let mut announced = DiagnosticCounts::default();
    let mut rebuilding = false;

    loop {
        let pending = updates.take();
        if pending.shutdown {
            handshakes.shutdown().await;
            close_clients(&mut clients).await;
            return;
        }
        if let Some(change) = pending.revision {
            let current_site = sites
                .revision()
                .expect("reload transport retains a current site");
            let current_revision = current_site.manifest().revision();
            let page_availability = current_site.page_availability();
            match change {
                RevisionChange::Changed(diff) => {
                    broadcast_revision(
                        &mut clients,
                        Some(&diff),
                        current_revision,
                        page_availability,
                    )
                    .await;
                }
                RevisionChange::Resync(_) => {
                    broadcast_revision(&mut clients, None, current_revision, page_availability)
                        .await;
                }
                RevisionChange::Unchanged => {}
            }
        }
        let counts_reported = pending.counts.is_some();
        if let Some(next) = pending.counts {
            indicator.build_replaced(next);
        }
        if let Some(failure) = pending.consumer {
            indicator.consumer_failed(failure);
        }
        // A failure of the revision that is still installed keeps counting; a replaced
        // revision's failure stops here, even when no round reported for its successor.
        let site = sites.revision();
        let counts = indicator.for_revision(site.as_ref());
        let counts_changed = counts != announced;
        announced = counts;
        let rebuilding_changed = pending.rebuilding.is_some_and(|next| next != rebuilding);
        rebuilding = pending.rebuilding.unwrap_or(rebuilding);
        if counts_reported || counts_changed || rebuilding_changed {
            broadcast(&mut clients, &HotReloadMessage::status(rebuilding, counts)).await;
        }
        while let Some(handshake) = handshakes.try_join_next() {
            admit_client(
                &mut clients,
                &sites,
                &indicator,
                rebuilding,
                handshake.expect("reload handshake task panicked"),
            )
            .await;
        }

        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, address)) => {
                        if clients.len() + handshakes.len() >= MAX_RELOAD_CLIENTS {
                            tracing::debug!(
                                target: "tola::reload",
                                limit = MAX_RELOAD_CLIENTS,
                                "reload client limit reached"
                            );
                            continue;
                        }
                        if handshakes.len() >= MAX_PENDING_HANDSHAKES {
                            tracing::debug!(
                                target: "tola::reload",
                                limit = MAX_PENDING_HANDSHAKES,
                                "reload handshake limit reached"
                            );
                            continue;
                        }
                        let session_token = Arc::clone(&session_token);
                        handshakes.spawn(async move {
                            handshake_client(stream, &session_token, address).await
                        });
                    }
                    Err(error) => {
                        tracing::debug!(target: "tola::reload", %error, "WebSocket accept failed");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    }
                }
            }
            result = handshakes.join_next(), if !handshakes.is_empty() => {
                if let Some(handshake) = result {
                    admit_client(
                        &mut clients,
                        &sites,
                        &indicator,
                        rebuilding,
                        handshake.expect("reload handshake task panicked"),
                    )
                    .await;
                }
            }
            _ = updates.available.notified() => {}
            _ = read_clients(&mut clients), if !clients.is_empty() => {}
        }
    }
}

async fn admit_client(
    clients: &mut ReloadClients,
    sites: &CurrentSite,
    indicator: &IndicatorCounts,
    rebuilding: bool,
    handshake: std::result::Result<(ClientStream, SocketAddr), SocketAddr>,
) {
    match handshake {
        Ok((mut client, address)) => {
            tracing::debug!(target: "tola::reload", %address, "client connected");
            let current_site = sites.revision();
            let revision = current_site
                .as_ref()
                .map(|site| site.manifest().revision().clone());
            let counts = indicator.for_revision(current_site.as_ref());
            if send_initial_state(&mut client, current_site.as_deref(), counts, rebuilding).await {
                clients.insert(
                    address,
                    ReloadClient {
                        socket: client,
                        delivered_revision: revision,
                    },
                );
            }
        }
        Err(address) => {
            tracing::debug!(target: "tola::reload", %address, "WebSocket handshake failed");
        }
    }
}

async fn handshake_client(
    stream: tokio::net::TcpStream,
    session_token: &str,
    address: SocketAddr,
) -> std::result::Result<(ClientStream, SocketAddr), SocketAddr> {
    let websocket_config = websocket_config();
    let handshake = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async_with_config(
            stream,
            SessionHandshake { session_token },
            Some(websocket_config),
        ),
    )
    .await;
    match handshake {
        Ok(Ok(client)) => Ok((client, address)),
        Ok(Err(error)) => {
            tracing::debug!(
                target: "tola::reload",
                %address,
                error = ?error,
                "WebSocket handshake worker failed"
            );
            Err(address)
        }
        Err(_elapsed) => {
            tracing::debug!(
                target: "tola::reload",
                %address,
                "WebSocket handshake timed out"
            );
            Err(address)
        }
    }
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(CLIENT_READ_BUFFER_BYTES)
        .write_buffer_size(SERVER_INITIAL_WRITE_BUFFER_BYTES)
        .max_write_buffer_size(SERVER_WRITE_BUFFER_BYTES)
        .max_message_size(Some(MAX_CLIENT_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_CLIENT_MESSAGE_BYTES))
}

fn new_session_id() -> Result<String> {
    let mut bytes = [0u8; SESSION_ID_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| {
        anyhow!("Tola could not start the browser reload session; restart `tola dev`")
    })?;
    Ok(hex::encode(bytes))
}

struct SessionHandshake<'a> {
    session_token: &'a str,
}

impl Callback for SessionHandshake<'_> {
    #[allow(clippy::result_large_err)]
    fn on_request(
        self,
        request: &Request,
        response: Response,
    ) -> std::result::Result<Response, ErrorResponse> {
        if valid_session_query(request.uri().query(), self.session_token)
            && origin_matches_host(request.headers())
        {
            Ok(response)
        } else {
            Err(forbidden_handshake())
        }
    }
}

fn valid_session_query(query: Option<&str>, session_token: &str) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut sessions = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(name, _)| name == "tola-session")
        .map(|(_, value)| value);
    matches!((sessions.next(), sessions.next()), (Some(value), None) if value.as_ref() == session_token)
}

fn origin_matches_host(headers: &HeaderMap) -> bool {
    let mut origins = headers.get_all("origin").iter();
    let Some(origin) = origins.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if origins.next().is_some() {
        return false;
    }
    let mut hosts = headers.get_all("host").iter();
    let Some(host) = hosts.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if hosts.next().is_some() {
        return false;
    }
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(origin.scheme(), "http" | "https")
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return false;
    }
    let Ok(authority) = host.parse::<tokio_tungstenite::tungstenite::http::uri::Authority>() else {
        return false;
    };
    origin
        .host_str()
        .is_some_and(|origin_host| origin_host.eq_ignore_ascii_case(authority.host()))
}

fn forbidden_handshake() -> ErrorResponse {
    tokio_tungstenite::tungstenite::http::Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Some("Forbidden".to_owned()))
        .expect("static WebSocket rejection is valid")
}

struct ReloadClient<S = tokio::net::TcpStream> {
    socket: WebSocketStream<S>,
    delivered_revision: Option<RevisionId>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Stream for ReloadClient<S> {
    type Item = tokio_tungstenite::tungstenite::Result<Message>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().socket).poll_next(cx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevisionDelivery {
    AdjacentDiff,
    AlreadyCurrent,
    Resync,
}

fn revision_delivery(
    client: Option<&RevisionId>,
    diff: Option<&RevisionDiff>,
    current: &RevisionId,
) -> RevisionDelivery {
    if client == Some(current) {
        RevisionDelivery::AlreadyCurrent
    } else if diff.is_some_and(|diff| diff.to() == current && client == Some(diff.from())) {
        RevisionDelivery::AdjacentDiff
    } else {
        RevisionDelivery::Resync
    }
}

async fn broadcast_revision<S: AsyncRead + AsyncWrite + Unpin>(
    clients: &mut ReloadClients<S>,
    diff: Option<&RevisionDiff>,
    current: &RevisionId,
    page_availability: PageAvailability,
) {
    let mut adjacent_message = None::<Bytes>;
    let mut resync_message = None::<Bytes>;
    // Admission bounds this batch; keep one in-flight send per client.
    let sends = FuturesUnordered::new();
    for (address, client) in clients.iter_mut() {
        let delivery = revision_delivery(client.delivered_revision.as_ref(), diff, current);
        let message = match delivery {
            RevisionDelivery::AdjacentDiff => Some(
                adjacent_message
                    .get_or_insert_with(|| {
                        HotReloadMessage::revision(
                            diff.expect("adjacent delivery requires a matching revision diff"),
                            page_availability,
                        )
                        .to_json()
                        .into()
                    })
                    .clone(),
            ),
            RevisionDelivery::AlreadyCurrent => None,
            RevisionDelivery::Resync => Some(
                resync_message
                    .get_or_insert_with(|| {
                        HotReloadMessage::connected(current, page_availability)
                            .to_json()
                            .into()
                    })
                    .clone(),
            ),
        };
        sends.push(async move {
            if let Some(message) = message {
                if !send_to_client(client, &message).await {
                    return Some(*address);
                }
                client.delivered_revision = Some(current.clone());
            }
            None
        });
    }
    let failed = sends
        .filter_map(std::future::ready)
        .collect::<Vec<_>>()
        .await;
    evict_failed_sends(clients, failed);
}

async fn read_clients<S: AsyncRead + AsyncWrite + Unpin>(clients: &mut ReloadClients<S>) {
    // Return to the mailbox and accept branches after one message; cooperative
    // budgeting also yields when a peer keeps its read buffer continuously ready.
    let Some((address, message)) = tokio::task::coop::cooperative(clients.next()).await else {
        return;
    };
    if matches!(
        message,
        Ok(Message::Close(_) | Message::Text(_) | Message::Binary(_)) | Err(_)
    ) {
        clients.remove(&address);
    }
}

async fn send_initial(client: &mut ClientStream, message: &HotReloadMessage<'_>) -> bool {
    send(client, &message.to_json().into()).await.is_ok()
}

async fn send_initial_state(
    client: &mut ClientStream,
    site: Option<&SiteRevision>,
    counts: DiagnosticCounts,
    rebuilding: bool,
) -> bool {
    let connected = match site {
        Some(site) => {
            send_initial(
                client,
                &HotReloadMessage::connected(site.manifest().revision(), site.page_availability()),
            )
            .await
        }
        None => send_initial(client, &HotReloadMessage::awaiting()).await,
    };
    connected && send_initial(client, &HotReloadMessage::status(rebuilding, counts)).await
}

async fn broadcast<S: AsyncRead + AsyncWrite + Unpin>(
    clients: &mut ReloadClients<S>,
    message: &HotReloadMessage<'_>,
) {
    let message = Bytes::from(message.to_json());
    let sends = FuturesUnordered::new();
    for (address, client) in clients.iter_mut() {
        let message = &message;
        sends.push(async move { (!send_to_client(client, message).await).then_some(*address) });
    }
    let failed = sends
        .filter_map(std::future::ready)
        .collect::<Vec<_>>()
        .await;
    evict_failed_sends(clients, failed);
}

fn evict_failed_sends<S: AsyncRead + AsyncWrite + Unpin>(
    clients: &mut ReloadClients<S>,
    failed: Vec<SocketAddr>,
) {
    for address in failed {
        clients.remove(&address);
    }
}

async fn close_clients<S: AsyncRead + AsyncWrite + Unpin>(clients: &mut ReloadClients<S>) {
    let mut closing = FuturesUnordered::new();
    for client in clients.values_mut() {
        closing.push(async move {
            let _ = tokio::time::timeout(CLIENT_WRITE_TIMEOUT, client.socket.close(None)).await;
        });
    }
    while closing.next().await.is_some() {}
    drop(closing);
    clients.clear();
}

async fn send_to_client<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut ReloadClient<S>,
    message: &Bytes,
) -> bool {
    match send(&mut client.socket, message).await {
        Ok(()) => true,
        Err(error) => {
            tracing::debug!(target: "tola::reload", %error, "dropping slow reload client");
            false
        }
    }
}

async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut WebSocketStream<S>,
    message: &Bytes,
) -> Result<()> {
    // One JSON text message, fragmented on the wire only. Shared slices avoid
    // copying the whole payload per client; flushing each bounded frame caps
    // tungstenite's private buffer.
    tokio::time::timeout(CLIENT_WRITE_TIMEOUT, async {
        let mut start = 0;
        loop {
            let end = (start + SERVER_FRAME_BYTES).min(message.len());
            let opcode = if start == 0 {
                Data::Text
            } else {
                Data::Continue
            };
            let finished = end == message.len();
            client
                .send(Message::Frame(Frame::message(
                    message.slice(start..end),
                    OpCode::Data(opcode),
                    finished,
                )))
                .await?;
            if finished {
                return Ok::<(), tokio_tungstenite::tungstenite::Error>(());
            }
            start = end;
        }
    })
    .await
    .map_err(|_| anyhow!("reload client write timed out"))?
    .map_err(anyhow::Error::from)
}

/// Bind the browser reload listener: the preferred port when it is free, otherwise a port the
/// system assigns, so two development servers on one machine each keep their reload channel.
fn bind_listener(interface: IpAddr, preferred_port: u16) -> Result<(TcpListener, u16)> {
    match bind_port(interface, preferred_port) {
        Ok(bound) => Ok(bound),
        Err(error) => {
            tracing::debug!(
                target: "tola::reload",
                %error,
                preferred_port,
                "the preferred reload port is unavailable; binding a system-assigned port"
            );
            bind_port(interface, 0)
                .map_err(|error| anyhow!("the browser reload port is unavailable: {error}"))
        }
    }
}

fn bind_port(interface: IpAddr, port: u16) -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(SocketAddr::new(interface, port))?;
    let bound = listener.local_addr()?.port();
    Ok((listener, bound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{FutureExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    struct ReloadSite {
        _directory: tempfile::TempDir,
        config: tola_build::config::ResolvedSiteConfig,
    }

    impl ReloadSite {
        fn new() -> Self {
            let directory = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(directory.path().join("content")).unwrap();
            let config = tola_build::config::SiteConfigSchema::default()
                .resolve(
                    &directory.path().join("tola.toml"),
                    tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                    &tola_build::config::loading::BuildOverrides::default(),
                )
                .unwrap();
            Self {
                _directory: directory,
                config,
            }
        }

        fn manifest(&self, body: Option<&[u8]>) -> tola_build::output::manifest::SiteManifest {
            let build = build(&self.config, body);
            build
                .into_unchecked_revision(None)
                .site()
                .manifest()
                .clone()
        }
    }

    fn write_source(config: &tola_build::config::ResolvedSiteConfig, body: Option<&[u8]>) {
        let source = if let Some(body) = body {
            std::fs::write(config.get_root().join("body.txt"), body).unwrap();
            r#"#document("index.html")[#read("body.txt")]"#
        } else {
            ""
        };
        std::fs::write(&config.build().entry, source).unwrap();
    }

    fn build(
        config: &tola_build::config::ResolvedSiteConfig,
        body: Option<&[u8]>,
    ) -> tola_build::build::SiteBuild {
        write_source(config, body);
        tola_build::build::build_site(config, tola_build::build::BuildMode::Development).unwrap()
    }

    type TestClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn read_json<S: AsyncRead + AsyncWrite + Unpin>(
        client: &mut WebSocketStream<S>,
    ) -> serde_json::Value {
        let message = client.next().await.unwrap().unwrap().into_text().unwrap();
        serde_json::from_str(&message).unwrap()
    }

    fn current_sites() -> (CurrentSite, ReloadSite) {
        let sites = CurrentSite::new();
        let site = ReloadSite::new();
        replace(&sites, &site.config, None);
        (sites, site)
    }

    fn status_message(rebuilding: bool) -> HotReloadMessage<'static> {
        HotReloadMessage::status(rebuilding, DiagnosticCounts::default())
    }

    #[tokio::test]
    async fn client_resyncs_after_first_revision() {
        let sites = CurrentSite::new();
        let site = ReloadSite::new();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        transport.replace_diagnostics(&[Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Error,
            "First build failed",
        )]);
        let mut client = connect(&transport).await;
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "awaiting"})
        );
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 1, "warnings": 0})
        );

        let revision = replace(&sites, &site.config, Some(b"Recovered"));
        transport.first_revision_ready(revision.clone(), &[]);

        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({
                "type": "connected", "revision": revision.as_str(), "page_availability": "present",
            })
        );
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 0, "warnings": 0})
        );
        transport.shutdown().await.unwrap();
    }

    /// Install through `session`, because a candidate from
    /// [`tola_build::build::build_site`] has no origin and
    /// [`CurrentSite::replace`] rejects it.
    fn replace(
        sites: &CurrentSite,
        config: &tola_build::config::ResolvedSiteConfig,
        body: Option<&[u8]>,
    ) -> RevisionId {
        write_source(config, body);
        let mut session = tola_build::build::BuildSession::new();
        let attempt = session.prepare(
            std::sync::Arc::new(config.clone()),
            tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Development),
        );
        let candidate = attempt.run().unwrap().into_unchecked_revision(None);
        let checked = candidate
            .check(&tola_build::cancellation::BuildCancellation::new())
            .unwrap()
            .into_checked()
            .unwrap();
        let (_, current) = sites.replace(&mut session, checked).unwrap();
        current.manifest().revision().clone()
    }

    fn client_request(
        transport: &ReloadTransport,
        session_token: &str,
        origin: &str,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut request = format!(
            "ws://127.0.0.1:{}/?tola-session={session_token}",
            transport.endpoint().port()
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        request
    }

    async fn connect(transport: &ReloadTransport) -> TestClient {
        tokio_tungstenite::connect_async(client_request(
            transport,
            transport.endpoint().session_token(),
            "http://127.0.0.1:8080",
        ))
        .await
        .unwrap()
        .0
    }

    #[test]
    fn listener_binds_configured_interface() {
        let interface = "0.0.0.0".parse().unwrap();
        let (listener, port) = bind_listener(interface, 0).unwrap();

        assert_eq!(listener.local_addr().unwrap().ip(), interface);
        assert_ne!(port, 0);
    }

    #[test]
    fn taken_reload_port_binds_another() {
        let interface: IpAddr = "127.0.0.1".parse().unwrap();
        let (held, taken) = bind_port(interface, 0).unwrap();

        let (listener, bound) = bind_listener(interface, taken).unwrap();

        assert_ne!(bound, taken);
        assert_eq!(listener.local_addr().unwrap().ip(), interface);
        drop(held);
    }

    #[test]
    fn revision_change_reports_output_diffs() {
        let site = ReloadSite::new();
        let manifest = site.manifest(None);

        assert!(matches!(
            RevisionChange::from_diff(manifest.diff(&manifest)),
            RevisionChange::Unchanged
        ));

        let after = site.manifest(Some(b"page"));
        assert!(matches!(
            RevisionChange::from_diff(manifest.diff(&after)),
            RevisionChange::Changed(_)
        ));
    }

    #[test]
    fn pending_revisions_coalesce_to_resync() {
        let site = ReloadSite::new();
        let first = site.manifest(Some(b"first"));
        let second = site.manifest(Some(b"second"));
        let third = site.manifest(Some(b"third"));
        let mut pending = PendingUpdates::default();
        pending.revision_replaced(first.diff(&second), DiagnosticCounts::default());
        pending.revision_replaced(second.diff(&third), DiagnosticCounts::default());

        assert!(matches!(
            &pending.revision,
            Some(RevisionChange::Resync(revision)) if revision == third.revision()
        ));
    }

    #[tokio::test]
    async fn every_client_learns_the_rebuild_state() {
        let (sites, _site) = current_sites();
        let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites).unwrap();
        let mut existing = connect(&transport).await;
        read_json(&mut existing).await;
        read_json(&mut existing).await;

        let rebuilding = transport.begin_rebuild();
        assert_eq!(
            read_json(&mut existing).await,
            serde_json::json!({"type": "status", "rebuilding": true, "errors": 0, "warnings": 0})
        );

        let mut connected_during_rebuild = connect(&transport).await;
        read_json(&mut connected_during_rebuild).await;
        assert_eq!(
            read_json(&mut connected_during_rebuild).await,
            serde_json::json!({"type": "status", "rebuilding": true, "errors": 0, "warnings": 0})
        );

        drop(rebuilding);
        let finished =
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 0, "warnings": 0});
        assert_eq!(read_json(&mut existing).await, finished);
        assert_eq!(read_json(&mut connected_during_rebuild).await, finished);
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn handshake_rejects_untrusted_requests() {
        let (sites, _site) = current_sites();
        let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites).unwrap();

        let missing_token = client_request(&transport, "wrong", "http://127.0.0.1:8080");
        let error = tokio_tungstenite::connect_async(missing_token)
            .await
            .unwrap_err();
        if !matches!(
            &error,
            tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::FORBIDDEN
        ) {
            let shutdown = transport.shutdown().await;
            panic!(
                "unexpected missing-token handshake error: {error:?}; transport shutdown: {shutdown:?}"
            );
        }

        let foreign_origin = client_request(
            &transport,
            transport.endpoint().session_token(),
            "https://example.com",
        );
        let error = tokio_tungstenite::connect_async(foreign_origin)
            .await
            .unwrap_err();
        if !matches!(
            &error,
            tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::FORBIDDEN
        ) {
            let shutdown = transport.shutdown().await;
            panic!(
                "unexpected foreign-origin handshake error: {error:?}; transport shutdown: {shutdown:?}"
            );
        }

        let mut accepted = connect(&transport).await;
        assert_eq!(read_json(&mut accepted).await["type"], "connected");
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn current_revision_client_skips_diff() {
        let (sites, site) = current_sites();
        let before = sites.revision().unwrap().manifest().clone();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        let mut existing = connect(&transport).await;
        read_json(&mut existing).await;
        read_json(&mut existing).await;

        let next_revision = replace(&sites, &site.config, Some(b"next"));
        let after = sites.revision().unwrap().manifest().clone();
        let mut loaded_after_http_update = connect(&transport).await;
        assert_eq!(
            read_json(&mut loaded_after_http_update).await,
            serde_json::json!({
                "type": "connected",
                "revision": next_revision.as_str(),
                "page_availability": "present",
            })
        );
        read_json(&mut loaded_after_http_update).await;

        transport.revision_replaced(before.diff(&after), &[]);

        assert_eq!(read_json(&mut existing).await["type"], "revision");
        assert_eq!(read_json(&mut existing).await["type"], "status");
        assert_eq!(
            read_json(&mut loaded_after_http_update).await["type"],
            "status"
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_diff_resyncs_to_current() {
        let (sites, site) = current_sites();
        replace(&sites, &site.config, Some(b"first"));
        let before = sites.revision().unwrap().manifest().clone();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        let mut client = connect(&transport).await;
        read_json(&mut client).await;
        read_json(&mut client).await;

        replace(&sites, &site.config, Some(b"second"));
        let intermediate = sites.revision().unwrap().manifest().clone();
        let current = replace(&sites, &site.config, None);
        transport.revision_replaced(before.diff(&intermediate), &[]);

        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({
                "type": "connected",
                "revision": current.as_str(),
                "page_availability": "empty",
            })
        );
        transport.shutdown().await.unwrap();
    }

    fn hook_failure() -> Diagnostic {
        Diagnostic::new(
            crate::codes::hook::AFTER_PUBLISH,
            tola_build::diagnostic::Severity::Error,
            "After-publish command failed",
        )
    }

    fn build_failure() -> Diagnostic {
        Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Error,
            "Page build failed",
        )
    }

    fn build_warning() -> Diagnostic {
        Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Warning,
            "Unused import",
        )
    }

    #[tokio::test]
    async fn identical_output_publication_clears_failure() {
        let sites = CurrentSite::new();
        let site = ReloadSite::new();
        replace(&sites, &site.config, Some(b"same"));
        let first = sites.revision().unwrap();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        let mut client = connect(&transport).await;
        read_json(&mut client).await;
        read_json(&mut client).await;

        transport
            .diagnostics_sender()
            .after_publish_failed(&first, &hook_failure());
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 1, "warnings": 0})
        );

        // No round reports for the replacement, so only the publication can clear the failure.
        replace(&sites, &site.config, Some(b"same"));
        let second = sites.revision().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.manifest().revision(), second.manifest().revision());
        transport.revision_replaced(first.manifest().diff(second.manifest()), &[build_warning()]);

        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 0, "warnings": 1})
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn new_client_status_omits_superseded_failure() {
        let (sites, site) = current_sites();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        let consumed = sites.revision().unwrap();
        replace(&sites, &site.config, Some(b"second"));
        let installed = sites.revision().unwrap();
        transport.revision_replaced(
            consumed.manifest().diff(installed.manifest()),
            &[build_warning()],
        );

        // The replaced revision's round reports after the new revision is installed.
        transport
            .diagnostics_sender()
            .after_publish_failed(&consumed, &hook_failure());

        let mut client = connect(&transport).await;
        assert_eq!(read_json(&mut client).await["type"], "connected");
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 0, "warnings": 1})
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn failed_build_keeps_consumer_failure() {
        let (sites, _site) = current_sites();
        let transport =
            ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
        let installed = sites.revision().unwrap();
        let mut client = connect(&transport).await;
        read_json(&mut client).await;
        read_json(&mut client).await;

        transport
            .diagnostics_sender()
            .after_publish_failed(&installed, &hook_failure());
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 1, "warnings": 0})
        );

        // The failed build publishes no revision, so the installed revision's failure stays.
        transport.replace_diagnostics(&[build_failure()]);
        assert_eq!(
            read_json(&mut client).await,
            serde_json::json!({"type": "status", "rebuilding": false, "errors": 2, "warnings": 0})
        );
        transport.shutdown().await.unwrap();
    }

    async fn socket_pair(
        capacity: usize,
    ) -> (
        ReloadClient<tokio::io::DuplexStream>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let (server, browser) = tokio::io::duplex(capacity);
        let server =
            WebSocketStream::from_raw_socket(server, Role::Server, Some(websocket_config())).await;
        let browser = WebSocketStream::from_raw_socket(browser, Role::Client, None).await;
        (
            ReloadClient {
                socket: server,
                delivered_revision: None,
            },
            browser,
        )
    }

    #[tokio::test]
    async fn reads_do_not_stall_broadcasts() {
        let closing_address = SocketAddr::from(([127, 0, 0, 1], 1));
        let healthy_address = SocketAddr::from(([127, 0, 0, 1], 2));
        let (closing, mut closing_browser) = socket_pair(1024).await;
        let (healthy, mut healthy_browser) = socket_pair(1024).await;
        let mut clients = ReloadClients::new();
        clients.insert(closing_address, closing);
        clients.insert(healthy_address, healthy);

        // A cancelled pending read must leave the same sockets available to write.
        assert!(read_clients(&mut clients).now_or_never().is_none());
        broadcast(&mut clients, &status_message(true)).await;
        assert_eq!(read_json(&mut closing_browser).await["rebuilding"], true);
        assert_eq!(read_json(&mut healthy_browser).await["rebuilding"], true);

        let ping = Bytes::from_static(b"ready");
        closing_browser
            .send(Message::Ping(ping.clone()))
            .await
            .unwrap();
        assert!(read_clients(&mut clients).now_or_never().is_some());
        // Polling the next read flushes tungstenite's automatic pong.
        assert!(read_clients(&mut clients).now_or_never().is_none());
        assert_eq!(
            closing_browser
                .next()
                .now_or_never()
                .expect("pong must be ready without advancing a timer")
                .unwrap()
                .unwrap(),
            Message::Pong(ping)
        );

        closing_browser.close(None).await.unwrap();
        assert!(read_clients(&mut clients).now_or_never().is_some());
        assert!(!clients.contains_key(&closing_address));
        broadcast(&mut clients, &status_message(false)).await;
        assert_eq!(read_json(&mut healthy_browser).await["rebuilding"], false);

        drop(healthy_browser);
        assert!(read_clients(&mut clients).now_or_never().is_some());
        assert!(clients.is_empty());
    }

    #[tokio::test]
    async fn large_revision_diff_arrives_whole() {
        let site = ReloadSite::new();
        let before = site.manifest(None);
        let source = (0..400)
            .map(|index| format!("#asset(\"images/item-{index}.txt\", \"content\")\n"))
            .collect::<String>();
        std::fs::write(&site.config.build().entry, source).unwrap();
        let built =
            tola_build::build::build_site(&site.config, tola_build::build::BuildMode::Development)
                .unwrap();
        let candidate = built.into_unchecked_revision(None);
        let after = candidate.site().manifest();
        let diff = before.diff(after);
        let expected = HotReloadMessage::revision(&diff, PageAvailability::Empty).to_json();
        assert!(expected.len() > SERVER_WRITE_BUFFER_BYTES);
        let (mut client, mut browser) = socket_pair(1024).await;
        client.delivered_revision = Some(before.revision().clone());
        let mut clients = ReloadClients::new();
        clients.insert(SocketAddr::from(([127, 0, 0, 1], 0)), client);
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                async {
                    broadcast_revision(
                        &mut clients,
                        Some(&diff),
                        after.revision(),
                        PageAvailability::Empty,
                    )
                    .await;
                    // A successful delivery must suppress the same revision next time.
                    broadcast_revision(
                        &mut clients,
                        Some(&diff),
                        after.revision(),
                        PageAvailability::Empty,
                    )
                    .await;
                    broadcast(&mut clients, &status_message(false)).await;
                },
                async {
                    assert_eq!(
                        read_json(&mut browser).await,
                        serde_json::from_str::<serde_json::Value>(&expected).unwrap()
                    );
                    assert_eq!(
                        read_json(&mut browser).await,
                        serde_json::json!({
                            "type": "status", "rebuilding": false, "errors": 0, "warnings": 0,
                        })
                    );
                }
            );
        })
        .await
        .unwrap();
    }

    /// Eight clients whose transport buffers are full, plus one healthy client at port 8.
    async fn blocked_clients() -> (
        ReloadClients<tokio::io::DuplexStream>,
        Vec<WebSocketStream<tokio::io::DuplexStream>>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        use tokio::io::AsyncWriteExt;
        let mut clients = ReloadClients::new();
        let mut blocked_browsers = Vec::new();
        for port in 0..8 {
            let (mut client, browser) = socket_pair(64).await;
            // Fill the bounded transport without relying on platform TCP buffers.
            client.socket.get_mut().write_all(&[0; 64]).await.unwrap();
            clients.insert(SocketAddr::from(([127, 0, 0, 1], port)), client);
            blocked_browsers.push(browser);
        }
        let (healthy, browser) = socket_pair(1024).await;
        clients.insert(SocketAddr::from(([127, 0, 0, 1], 8)), healthy);
        (clients, blocked_browsers, browser)
    }

    #[tokio::test]
    async fn blocked_clients_never_delay_delivery() {
        let (mut clients, blocked_browsers, mut browser) = blocked_clients().await;
        let status = status_message(true);
        tokio::time::timeout(CLIENT_WRITE_TIMEOUT * 3, async {
            tokio::join!(broadcast(&mut clients, &status), async {
                let received =
                    tokio::time::timeout(CLIENT_WRITE_TIMEOUT / 2, read_json(&mut browser))
                        .await
                        .unwrap();
                assert_eq!(
                    received,
                    serde_json::from_str::<serde_json::Value>(&status.to_json()).unwrap()
                );
            });
        })
        .await
        .unwrap();
        assert_eq!(clients.len(), 1);
        let idle = status_message(false);
        tokio::join!(broadcast(&mut clients, &idle), async {
            assert_eq!(read_json(&mut browser).await["rebuilding"], false);
        });
        drop(blocked_browsers);
    }

    #[tokio::test]
    async fn blocked_clients_share_one_deadline() {
        let (mut clients, blocked_browsers, mut browser) = blocked_clients().await;
        tokio::time::timeout(CLIENT_WRITE_TIMEOUT * 3, async {
            tokio::join!(close_clients(&mut clients), async {
                let received = tokio::time::timeout(CLIENT_WRITE_TIMEOUT / 2, browser.next())
                    .await
                    .unwrap();
                assert!(matches!(received, Some(Ok(Message::Close(_)))));
            });
        })
        .await
        .unwrap();
        assert!(clients.is_empty());
        drop(blocked_browsers);
    }
}
