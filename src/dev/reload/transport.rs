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
use tola_build::output::PageAvailability;
use tola_build::output::manifest::{RevisionDiff, RevisionId};
use tola_build::site::SiteRevision;
use tola_diagnostic::Diagnostic;

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
    session_token: Arc<str>,
    generation: Arc<str>,
}

impl ReloadEndpoint {
    pub(crate) fn new(
        port: u16,
        session_token: impl Into<Arc<str>>,
        generation: impl Into<Arc<str>>,
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

impl RevisionChange {
    fn from_diff(diff: RevisionDiff) -> Self {
        if diff.is_unchanged() {
            Self::Unchanged
        } else {
            Self::Changed(diff)
        }
    }
}

#[derive(Default)]
struct PendingUpdates {
    revision: Option<RevisionChange>,
    counts: Option<DiagnosticCounts>,
    consumer: Option<ConsumerFailure>,
    rebuilding: Option<bool>,
    shutdown: bool,
}

impl PendingUpdates {
    fn revision_replaced(&mut self, diff: RevisionDiff, counts: DiagnosticCounts) {
        let next = RevisionChange::from_diff(diff);
        self.revision = Some(match (self.revision.take(), next) {
            (None, next) | (Some(RevisionChange::Unchanged), next) => next,
            (Some(prev @ RevisionChange::Changed(_)), RevisionChange::Unchanged)
            | (Some(prev @ RevisionChange::Resync(_)), RevisionChange::Unchanged) => prev,
            (Some(RevisionChange::Changed(prev)), RevisionChange::Changed(next)) => {
                assert_eq!(
                    prev.to(),
                    next.from(),
                    "queued revision diffs must be contiguous"
                );
                RevisionChange::Resync(next.to().clone())
            }
            (Some(RevisionChange::Resync(prev)), RevisionChange::Changed(next)) => {
                assert_eq!(
                    &prev,
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

struct ConsumerFailure {
    /// Weak so a superseded site is not kept alive by its failure count.
    revision: Weak<SiteRevision>,
    counts: DiagnosticCounts,
}

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

    fn for_revision(&self, rev: Option<&Arc<SiteRevision>>) -> DiagnosticCounts {
        let mut counts = self.build;
        if let Some(failure) = &self.consumer
            && rev.is_some_and(|rev| Weak::ptr_eq(&failure.revision, &Arc::downgrade(rev)))
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
    pub(crate) fn after_publish_failed(&self, rev: &Arc<SiteRevision>, diag: &Diagnostic) {
        let mut counts = DiagnosticCounts::default();
        counts.count(diag);
        self.0.consumer_failed(ConsumerFailure {
            revision: Arc::downgrade(rev),
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

    fn first_revision_ready(&self, rev: RevisionId, counts: DiagnosticCounts) {
        self.publish(|pending| {
            pending.revision = Some(RevisionChange::Resync(rev));
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

    pub(crate) fn first_revision_ready(&self, rev: RevisionId, diags: &[Diagnostic]) {
        self.updates
            .first_revision_ready(rev, DiagnosticCounts::of(diags));
    }

    pub(crate) fn revision_replaced(&self, diff: RevisionDiff, diags: &[Diagnostic]) {
        self.updates
            .revision_replaced(diff, DiagnosticCounts::of(diags));
    }

    pub(crate) fn replace_diagnostics(&self, diags: &[Diagnostic]) {
        self.updates.counts_replaced(DiagnosticCounts::of(diags));
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
            let cur_site = sites
                .revision()
                .expect("reload transport retains a current site");
            let cur_rev = cur_site.manifest().revision();
            let page_avail = cur_site.page_availability();
            match change {
                RevisionChange::Changed(diff) => {
                    broadcast_revision(&mut clients, Some(&diff), cur_rev, page_avail).await;
                }
                RevisionChange::Resync(_) => {
                    broadcast_revision(&mut clients, None, cur_rev, page_avail).await;
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
                    Ok((stream, addr)) => {
                        if clients.len() + handshakes.len() >= MAX_RELOAD_CLIENTS {
                            tracing::debug!(target: "tola::reload", limit = MAX_RELOAD_CLIENTS, "reload client limit reached");
                            continue;
                        }
                        if handshakes.len() >= MAX_PENDING_HANDSHAKES {
                            tracing::debug!(target: "tola::reload", limit = MAX_PENDING_HANDSHAKES, "reload handshake limit reached");
                            continue;
                        }
                        let token = Arc::clone(&session_token);
                        handshakes.spawn(async move {
                            handshake_client(stream, &token, addr).await
                        });
                    }
                    Err(err) => {
                        tracing::debug!(target: "tola::reload", %err, "WebSocket accept failed");
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
        Ok((mut client, addr)) => {
            tracing::debug!(target: "tola::reload", %addr, "client connected");
            let cur_site = sites.revision();
            let rev = cur_site
                .as_ref()
                .map(|site| site.manifest().revision().clone());
            let counts = indicator.for_revision(cur_site.as_ref());
            if send_initial_state(&mut client, cur_site.as_deref(), counts, rebuilding).await {
                clients.insert(
                    addr,
                    ReloadClient {
                        socket: client,
                        delivered_revision: rev,
                    },
                );
            }
        }
        Err(addr) => {
            tracing::debug!(target: "tola::reload", %addr, "WebSocket handshake failed");
        }
    }
}

async fn handshake_client(
    stream: tokio::net::TcpStream,
    session_token: &str,
    addr: SocketAddr,
) -> std::result::Result<(ClientStream, SocketAddr), SocketAddr> {
    let ws_cfg = websocket_config();
    let handshake = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async_with_config(
            stream,
            SessionHandshake { session_token },
            Some(ws_cfg),
        ),
    )
    .await;
    match handshake {
        Ok(Ok(client)) => Ok((client, addr)),
        Ok(Err(err)) => {
            tracing::debug!(target: "tola::reload", %addr, error = ?err, "WebSocket handshake worker failed");
            Err(addr)
        }
        Err(_) => {
            tracing::debug!(target: "tola::reload", %addr, "WebSocket handshake timed out");
            Err(addr)
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
    let mut buf = [0u8; SESSION_ID_BYTES];
    getrandom::fill(&mut buf).map_err(|_| {
        anyhow!("Tola could not start the browser reload session; restart `tola dev`")
    })?;
    Ok(hex::encode(buf))
}

struct SessionHandshake<'a> {
    session_token: &'a str,
}

impl Callback for SessionHandshake<'_> {
    fn on_request(
        self,
        req: &Request,
        resp: Response,
    ) -> std::result::Result<Response, ErrorResponse> {
        if valid_session_query(req.uri().query(), self.session_token)
            && origin_matches_host(req.headers())
        {
            Ok(resp)
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
        .map(|(_, val)| val);
    matches!((sessions.next(), sessions.next()), (Some(val), None) if val.as_ref() == session_token)
}

fn single_header<'a>(hdrs: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut vals = hdrs.get_all(name).iter();
    let val = vals.next()?.to_str().ok()?;
    vals.next().is_none().then_some(val)
}

fn origin_matches_host(hdrs: &HeaderMap) -> bool {
    let (Some(origin), Some(host)) = (single_header(hdrs, "origin"), single_header(hdrs, "host"))
    else {
        return false;
    };
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
    cur: &RevisionId,
) -> RevisionDelivery {
    if client == Some(cur) {
        RevisionDelivery::AlreadyCurrent
    } else if diff.is_some_and(|diff| diff.to() == cur && client == Some(diff.from())) {
        RevisionDelivery::AdjacentDiff
    } else {
        RevisionDelivery::Resync
    }
}

async fn broadcast_revision<S: AsyncRead + AsyncWrite + Unpin>(
    clients: &mut ReloadClients<S>,
    diff: Option<&RevisionDiff>,
    cur: &RevisionId,
    page_avail: PageAvailability,
) {
    let mut adjacent_msg = None::<Bytes>;
    let mut resync_msg = None::<Bytes>;
    let sends = FuturesUnordered::new();
    for (addr, client) in clients.iter_mut() {
        let msg = match revision_delivery(client.delivered_revision.as_ref(), diff, cur) {
            RevisionDelivery::AdjacentDiff => Some(
                adjacent_msg
                    .get_or_insert_with(|| {
                        HotReloadMessage::revision(
                            diff.expect("adjacent delivery requires a matching revision diff"),
                            page_avail,
                        )
                        .to_json()
                        .into()
                    })
                    .clone(),
            ),
            RevisionDelivery::AlreadyCurrent => None,
            RevisionDelivery::Resync => Some(
                resync_msg
                    .get_or_insert_with(|| {
                        HotReloadMessage::connected(cur, page_avail)
                            .to_json()
                            .into()
                    })
                    .clone(),
            ),
        };
        sends.push(async move {
            if let Some(msg) = msg {
                if !send_to_client(client, &msg).await {
                    return Some(*addr);
                }
                client.delivered_revision = Some(cur.clone());
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
    let Some((addr, msg)) = tokio::task::coop::cooperative(clients.next()).await else {
        return;
    };
    if matches!(
        msg,
        Ok(Message::Close(_) | Message::Text(_) | Message::Binary(_)) | Err(_)
    ) {
        clients.remove(&addr);
    }
}

async fn send_initial(client: &mut ClientStream, msg: &HotReloadMessage<'_>) -> bool {
    send(client, &msg.to_json().into()).await.is_ok()
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
    msg: &HotReloadMessage<'_>,
) {
    let payload = Bytes::from(msg.to_json());
    let sends = FuturesUnordered::new();
    for (addr, client) in clients.iter_mut() {
        let payload = &payload;
        sends.push(async move { (!send_to_client(client, payload).await).then_some(*addr) });
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
    for addr in failed {
        clients.remove(&addr);
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
    msg: &Bytes,
) -> bool {
    match send(&mut client.socket, msg).await {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(target: "tola::reload", %err, "dropping slow reload client");
            false
        }
    }
}

async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut WebSocketStream<S>,
    msg: &Bytes,
) -> Result<()> {
    // Fragment as shared slices to avoid per-client payload copies and bound write buffers.
    tokio::time::timeout(CLIENT_WRITE_TIMEOUT, async {
        let mut start = 0;
        loop {
            let end = (start + SERVER_FRAME_BYTES).min(msg.len());
            let opcode = if start == 0 {
                Data::Text
            } else {
                Data::Continue
            };
            let finished = end == msg.len();
            client
                .send(Message::Frame(Frame::message(
                    msg.slice(start..end),
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

fn bind_listener(interface: IpAddr, preferred_port: u16) -> Result<(TcpListener, u16)> {
    match bind_port(interface, preferred_port) {
        Ok(bound) => Ok(bound),
        Err(err) => {
            tracing::debug!(
                target: "tola::reload",
                %err,
                preferred_port,
                "preferred reload port unavailable; binding system-assigned port"
            );
            bind_port(interface, 0)
                .map_err(|err| anyhow!("the browser reload port is unavailable: {err}"))
        }
    }
}

fn bind_port(interface: IpAddr, port: u16) -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(SocketAddr::new(interface, port))?;
    let bound = listener.local_addr()?.port();
    Ok((listener, bound))
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
