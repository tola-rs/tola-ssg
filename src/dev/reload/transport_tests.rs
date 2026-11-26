use super::*;
use futures_util::{FutureExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct ReloadSite {
    _dir: tempfile::TempDir,
    cfg: Arc<tola_build::config::SiteConfig>,
}

impl ReloadSite {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            cfg: crate::dev::tests::write_site(dir.path(), "", ""),
            _dir: dir,
        }
    }

    fn manifest(&self, body: Option<&[u8]>) -> tola_build::output::manifest::SiteManifest {
        crate::dev::tests::write_source(&self.cfg, body);
        crate::dev::tests::built_outputs(&self.cfg, tola_build::build::BuildMode::Development)
            .manifest()
            .clone()
    }
}

type TestClient = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn read_json<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut WebSocketStream<S>,
) -> serde_json::Value {
    let msg = client.next().await.unwrap().unwrap().into_text().unwrap();
    serde_json::from_str(&msg).unwrap()
}

fn current_sites() -> (CurrentSite, ReloadSite) {
    let sites = CurrentSite::new();
    let site = ReloadSite::new();
    replace(&sites, &site.cfg, None);
    (sites, site)
}

fn status_message(rebuilding: bool) -> HotReloadMessage<'static> {
    HotReloadMessage::status(rebuilding, DiagnosticCounts::default())
}

fn status_frame(rebuilding: bool, errors: usize, warnings: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "status",
        "rebuilding": rebuilding,
        "errors": errors,
        "warnings": warnings,
    })
}

fn replace(
    sites: &CurrentSite,
    cfg: &tola_build::config::SiteConfig,
    body: Option<&[u8]>,
) -> RevisionId {
    crate::dev::tests::write_source(cfg, body);
    let mut session = tola_build::build::BuildSession::new();
    let (checked, _) = crate::dev::tests::checked_candidate(
        &mut session,
        Arc::new(cfg.clone()),
        tola_build::build::BuildMode::Development,
    );
    let (_, cur) = sites.replace(&mut session, checked).unwrap();
    cur.manifest().revision().clone()
}

fn client_request(
    transport: &ReloadTransport,
    session_token: &str,
    origin: &str,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut req = format!(
        "ws://127.0.0.1:{}/?tola-session={session_token}",
        transport.endpoint().port()
    )
    .into_client_request()
    .unwrap();
    req.headers_mut().insert("Origin", origin.parse().unwrap());
    req
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

async fn assert_handshake_forbidden(req: tokio_tungstenite::tungstenite::http::Request<()>) {
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(
        matches!(
            &err,
            tokio_tungstenite::tungstenite::Error::Http(resp)
                if resp.status() == StatusCode::FORBIDDEN
        ),
        "unexpected handshake error: {err:?}"
    );
}

#[tokio::test]
async fn client_resync_first_revision() {
    let sites = CurrentSite::new();
    let site = ReloadSite::new();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    transport.replace_diagnostics(&[Diagnostic::new(
        tola_diagnostic::codes::typst::COMPILE,
        tola_diagnostic::Severity::Error,
        "First build failed",
    )]);
    let mut client = connect(&transport).await;
    assert_eq!(
        read_json(&mut client).await,
        serde_json::json!({"type": "awaiting"})
    );
    assert_eq!(read_json(&mut client).await, status_frame(false, 1, 0));

    let rev = replace(&sites, &site.cfg, Some(b"Recovered"));
    transport.first_revision_ready(rev.clone(), &[]);

    assert_eq!(
        read_json(&mut client).await,
        serde_json::json!({
            "type": "connected", "revision": rev.as_str(), "page_availability": "present",
        })
    );
    assert_eq!(read_json(&mut client).await, status_frame(false, 0, 0));
    transport.shutdown().await.unwrap();
}

#[test]
fn listener_binds_interface() {
    let interface = "0.0.0.0".parse().unwrap();
    let (listener, port) = bind_listener(interface, 0).unwrap();

    assert_eq!(listener.local_addr().unwrap().ip(), interface);
    assert_ne!(port, 0);
}

#[test]
fn taken_port_fallback() {
    let interface: IpAddr = "127.0.0.1".parse().unwrap();
    let (held, taken) = bind_port(interface, 0).unwrap();

    let (listener, bound) = bind_listener(interface, taken).unwrap();

    assert_ne!(bound, taken);
    assert_eq!(listener.local_addr().unwrap().ip(), interface);
    drop(held);
}

#[test]
fn revision_change_diffs() {
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
fn pending_revisions_coalesce() {
    let site = ReloadSite::new();
    let first = site.manifest(Some(b"first"));
    let second = site.manifest(Some(b"second"));
    let third = site.manifest(Some(b"third"));
    let mut pending = PendingUpdates::default();
    pending.revision_replaced(first.diff(&second), DiagnosticCounts::default());
    pending.revision_replaced(second.diff(&third), DiagnosticCounts::default());

    assert!(matches!(
        &pending.revision,
        Some(RevisionChange::Resync(rev)) if rev == third.revision()
    ));
}

#[tokio::test]
async fn clients_receive_rebuild_state() {
    let (sites, _site) = current_sites();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites).unwrap();
    let mut existing = connect(&transport).await;
    read_json(&mut existing).await;
    read_json(&mut existing).await;

    let rebuilding = transport.begin_rebuild();
    assert_eq!(read_json(&mut existing).await, status_frame(true, 0, 0));

    let mut during = connect(&transport).await;
    read_json(&mut during).await;
    assert_eq!(read_json(&mut during).await, status_frame(true, 0, 0));

    drop(rebuilding);
    let finished = status_frame(false, 0, 0);
    assert_eq!(read_json(&mut existing).await, finished);
    assert_eq!(read_json(&mut during).await, finished);
    transport.shutdown().await.unwrap();
}

#[tokio::test]
async fn handshake_untrusted_rejected() {
    let (sites, _site) = current_sites();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites).unwrap();

    assert_handshake_forbidden(client_request(&transport, "wrong", "http://127.0.0.1:8080")).await;
    assert_handshake_forbidden(client_request(
        &transport,
        transport.endpoint().session_token(),
        "https://example.com",
    ))
    .await;

    let mut accepted = connect(&transport).await;
    assert_eq!(read_json(&mut accepted).await["type"], "connected");
    transport.shutdown().await.unwrap();
}

#[tokio::test]
async fn current_client_skips_diff() {
    let (sites, site) = current_sites();
    let before = sites.revision().unwrap().manifest().clone();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    let mut existing = connect(&transport).await;
    read_json(&mut existing).await;
    read_json(&mut existing).await;

    let next_rev = replace(&sites, &site.cfg, Some(b"next"));
    let after = sites.revision().unwrap().manifest().clone();
    let mut loaded_after = connect(&transport).await;
    assert_eq!(
        read_json(&mut loaded_after).await,
        serde_json::json!({
            "type": "connected",
            "revision": next_rev.as_str(),
            "page_availability": "present",
        })
    );
    read_json(&mut loaded_after).await;

    transport.revision_replaced(before.diff(&after), &[]);

    assert_eq!(read_json(&mut existing).await["type"], "revision");
    assert_eq!(read_json(&mut existing).await["type"], "status");
    assert_eq!(read_json(&mut loaded_after).await["type"], "status");
    transport.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_diff_resyncs() {
    let (sites, site) = current_sites();
    replace(&sites, &site.cfg, Some(b"first"));
    let before = sites.revision().unwrap().manifest().clone();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    let mut client = connect(&transport).await;
    read_json(&mut client).await;
    read_json(&mut client).await;

    replace(&sites, &site.cfg, Some(b"second"));
    let intermediate = sites.revision().unwrap().manifest().clone();
    let cur = replace(&sites, &site.cfg, None);
    transport.revision_replaced(before.diff(&intermediate), &[]);

    assert_eq!(
        read_json(&mut client).await,
        serde_json::json!({
            "type": "connected",
            "revision": cur.as_str(),
            "page_availability": "empty",
        })
    );
    transport.shutdown().await.unwrap();
}

fn hook_failure() -> Diagnostic {
    Diagnostic::new(
        crate::codes::hook::AFTER_PUBLISH,
        tola_diagnostic::Severity::Error,
        "After-publish command failed",
    )
}

fn build_failure() -> Diagnostic {
    Diagnostic::new(
        tola_diagnostic::codes::typst::COMPILE,
        tola_diagnostic::Severity::Error,
        "Page build failed",
    )
}

fn build_warning() -> Diagnostic {
    Diagnostic::new(
        tola_diagnostic::codes::typst::COMPILE,
        tola_diagnostic::Severity::Warning,
        "Unused import",
    )
}

#[tokio::test]
async fn identical_publication_clears_failure() {
    let sites = CurrentSite::new();
    let site = ReloadSite::new();
    replace(&sites, &site.cfg, Some(b"same"));
    let first = sites.revision().unwrap();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    let mut client = connect(&transport).await;
    read_json(&mut client).await;
    read_json(&mut client).await;

    transport
        .diagnostics_sender()
        .after_publish_failed(&first, &hook_failure());
    assert_eq!(read_json(&mut client).await, status_frame(false, 1, 0));

    replace(&sites, &site.cfg, Some(b"same"));
    let second = sites.revision().unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(first.manifest().revision(), second.manifest().revision());
    transport.revision_replaced(first.manifest().diff(second.manifest()), &[build_warning()]);

    assert_eq!(read_json(&mut client).await, status_frame(false, 0, 1));
    transport.shutdown().await.unwrap();
}

#[tokio::test]
async fn new_client_omits_superseded_failure() {
    let (sites, site) = current_sites();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    let consumed = sites.revision().unwrap();
    replace(&sites, &site.cfg, Some(b"second"));
    let installed = sites.revision().unwrap();
    transport.revision_replaced(
        consumed.manifest().diff(installed.manifest()),
        &[build_warning()],
    );

    transport
        .diagnostics_sender()
        .after_publish_failed(&consumed, &hook_failure());

    let mut client = connect(&transport).await;
    assert_eq!(read_json(&mut client).await["type"], "connected");
    assert_eq!(read_json(&mut client).await, status_frame(false, 0, 1));
    transport.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_build_retains_consumer_failure() {
    let (sites, _site) = current_sites();
    let transport = ReloadTransport::start("127.0.0.1".parse().unwrap(), 0, sites.clone()).unwrap();
    let installed = sites.revision().unwrap();
    let mut client = connect(&transport).await;
    read_json(&mut client).await;
    read_json(&mut client).await;

    transport
        .diagnostics_sender()
        .after_publish_failed(&installed, &hook_failure());
    assert_eq!(read_json(&mut client).await, status_frame(false, 1, 0));

    transport.replace_diagnostics(&[build_failure()]);
    assert_eq!(read_json(&mut client).await, status_frame(false, 2, 0));
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
async fn reads_nonblocking_broadcast() {
    let closing_addr = SocketAddr::from(([127, 0, 0, 1], 1));
    let healthy_addr = SocketAddr::from(([127, 0, 0, 1], 2));
    let (closing, mut closing_browser) = socket_pair(1024).await;
    let (healthy, mut healthy_browser) = socket_pair(1024).await;
    let mut clients = ReloadClients::new();
    clients.insert(closing_addr, closing);
    clients.insert(healthy_addr, healthy);

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
    assert!(read_clients(&mut clients).now_or_never().is_none());
    assert_eq!(
        closing_browser
            .next()
            .now_or_never()
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Pong(ping)
    );

    closing_browser.close(None).await.unwrap();
    assert!(read_clients(&mut clients).now_or_never().is_some());
    assert!(!clients.contains_key(&closing_addr));
    broadcast(&mut clients, &status_message(false)).await;
    assert_eq!(read_json(&mut healthy_browser).await["rebuilding"], false);

    drop(healthy_browser);
    assert!(read_clients(&mut clients).now_or_never().is_some());
    assert!(clients.is_empty());
}

#[tokio::test]
async fn large_diff_fragmented_whole() {
    let site = ReloadSite::new();
    let before = site.manifest(None);
    let src = (0..400)
        .map(|i| format!("#asset(\"images/item-{i}.txt\", \"content\")\n"))
        .collect::<String>();
    std::fs::write(&site.cfg.build().entry, src).unwrap();
    let built = tola_build::build::build_site(&site.cfg, tola_build::build::BuildMode::Development)
        .unwrap();
    let candidate = built.into_unchecked_revision(None);
    let after = candidate.site().manifest();
    let diff = before.diff(after);
    let expected = HotReloadMessage::revision(&diff, PageAvailability::Empty).to_json();
    assert!(expected.len() > SERVER_WRITE_BUFFER_BYTES);
    let (mut client, mut browser) = socket_pair(expected.len() + 1024).await;
    client.delivered_revision = Some(before.revision().clone());
    let mut clients = ReloadClients::new();
    clients.insert(SocketAddr::from(([127, 0, 0, 1], 0)), client);
    tokio::time::timeout(Duration::from_secs(60), async {
        tokio::join!(
            async {
                broadcast_revision(
                    &mut clients,
                    Some(&diff),
                    after.revision(),
                    PageAvailability::Empty,
                )
                .await;
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
                assert_eq!(read_json(&mut browser).await, status_frame(false, 0, 0));
            }
        );
    })
    .await
    .unwrap();
}

async fn blocked_clients() -> (
    ReloadClients<tokio::io::DuplexStream>,
    Vec<WebSocketStream<tokio::io::DuplexStream>>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    use tokio::io::AsyncWriteExt;
    let mut clients = ReloadClients::new();
    let mut blocked = Vec::new();
    for port in 0..8 {
        let (mut client, browser) = socket_pair(64).await;
        client.socket.get_mut().write_all(&[0; 64]).await.unwrap();
        clients.insert(SocketAddr::from(([127, 0, 0, 1], port)), client);
        blocked.push(browser);
    }
    let (healthy, browser) = socket_pair(1024).await;
    clients.insert(SocketAddr::from(([127, 0, 0, 1], 8)), healthy);
    (clients, blocked, browser)
}

#[tokio::test]
async fn blocked_clients_isolated() {
    let (mut clients, blocked, mut browser) = blocked_clients().await;
    let status = status_message(true);
    tokio::time::timeout(CLIENT_WRITE_TIMEOUT * 3, async {
        tokio::join!(broadcast(&mut clients, &status), async {
            let received = tokio::time::timeout(CLIENT_WRITE_TIMEOUT / 2, read_json(&mut browser))
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
    drop(blocked);
}

#[tokio::test]
async fn blocked_clients_shared_deadline() {
    let (mut clients, blocked, mut browser) = blocked_clients().await;
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
    drop(blocked);
}
