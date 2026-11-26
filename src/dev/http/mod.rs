//! HTTP listeners and connections for development revisions.

pub(super) mod html;
mod preview;
pub(super) mod response;

use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use hyper::{Request, body::Incoming};
use tola_address::RESERVED_ROOT;
use tola_diagnostic::{Diagnostic, Severity};

use crate::dev::site::CurrentSite;

pub(super) use preview::PublicationRequest;

const MAX_PORT_RETRIES: u16 = 10;
const MAX_HTTP_CONNECTIONS: usize = 256;
const HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_IDLE_CONNECTION_LIFETIME: Duration = Duration::from_secs(60);

/// Bind a nonblocking listener, trying consecutive ports when needed.
pub(super) fn bind_with_port_fallback(
    interface: std::net::IpAddr,
    base_port: u16,
) -> Result<(TcpListener, SocketAddr, Option<Diagnostic>)> {
    for offset in 0..MAX_PORT_RETRIES {
        let port = base_port.saturating_add(offset);
        let addr = SocketAddr::new(interface, port);
        let Ok((listener, addr)) = TcpListener::bind(addr).and_then(|listener| {
            listener.set_nonblocking(true)?;
            let addr = listener.local_addr()?;
            Ok((listener, addr))
        }) else {
            continue;
        };
        let fallback = (offset > 0).then(|| {
            Diagnostic::new(
                crate::codes::server::PORT_IN_USE,
                Severity::Warning,
                format!("port `{base_port}` is already in use"),
            )
            .with_note(format!("serving http://{addr} instead"))
            .with_help(format!(
                "Pass `--port {}` to use this port next time",
                addr.port()
            ))
        });
        return Ok((listener, addr, fallback));
    }
    let msg = format!("Tola could not start the development server on {interface}");
    let diag = Diagnostic::new(crate::codes::command::FAILED, Severity::Error, msg.clone())
        .with_help("Choose another address or port with `--interface`/`--port`");
    Err(tola_diagnostic::DiagnosticError::new(msg, vec![diag]).into())
}

pub(super) struct HttpServices {
    pub(super) listener: tokio::net::TcpListener,
    pub(super) sites: CurrentSite,
    pub(super) initial_config: Arc<tola_build::config::SiteConfig>,
    pub(super) reload_endpoint: Option<crate::dev::reload::transport::ReloadEndpoint>,
    pub(super) publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
    pub(super) shutdown: crate::cancellation::Cancellation,
    pub(super) events: crate::dev::events::Events,
    pub(super) url: String,
}

pub(super) async fn serve_http(services: HttpServices) -> Result<()> {
    let HttpServices {
        listener,
        sites,
        initial_config,
        reload_endpoint,
        publications,
        shutdown,
        events,
        url,
    } = services;
    if shutdown.is_requested() {
        return Ok(());
    }
    let rev = sites.revision();
    events.server_ready(
        &url,
        rev.as_ref().map(|site| site.manifest().revision().as_str()),
    )?;
    let mut connections = tokio::task::JoinSet::new();
    let conn_limit = Arc::new(tokio::sync::Semaphore::new(MAX_HTTP_CONNECTIONS));
    let serving = async {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                conn = listener.accept() => {
                    let (stream, addr) = match conn {
                        Ok(conn) => conn,
                        Err(err) => {
                            tracing::debug!(target: "tola::dev", %err, "accept failed");
                            return Err(anyhow!(
                                "Tola could not accept a development request; restart `tola dev`"
                            ));
                        }
                    };
                    let Ok(permit) = Arc::clone(&conn_limit).try_acquire_owned() else {
                        tracing::debug!(target: "tola::dev", %addr, limit = MAX_HTTP_CONNECTIONS, "HTTP connection limit reached");
                        continue;
                    };
                    let served = ServedSite {
                        sites: sites.clone(),
                        initial_config: Arc::clone(&initial_config),
                        reload_endpoint: reload_endpoint.clone(),
                        publications: publications.clone(),
                    };
                    let shutdown = shutdown.clone();
                    connections.spawn(async move {
                        let _permit = permit;
                        if let Err(err) = serve_connection(
                            stream,
                            addr,
                            served,
                            HTTP_IDLE_CONNECTION_LIFETIME,
                            shutdown.clone(),
                        )
                        .await
                            && !shutdown.is_requested()
                        {
                            tracing::debug!(target: "tola::dev", %err, "connection error");
                        }
                    });
                }
                Some(completion) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(err) = completion {
                        tracing::debug!(target: "tola::dev", %err, "connection task failed");
                        return Err(anyhow!(
                            "Tola could not serve a development request; restart `tola dev`"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    serving
}

const PREVIEW_ENDPOINT: &str = "preview";

/// Cross-language URL shared with the VS Code extension and preview e2e suite.
static PREVIEW_ENDPOINT_PATH: LazyLock<String> =
    LazyLock::new(|| format!("/{RESERVED_ROOT}/{PREVIEW_ENDPOINT}"));

struct ServedSite {
    sites: CurrentSite,
    initial_config: Arc<tola_build::config::SiteConfig>,
    reload_endpoint: Option<crate::dev::reload::transport::ReloadEndpoint>,
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
}

async fn serve_connection(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    peer: SocketAddr,
    served: ServedSite,
    idle_lifetime: Duration,
    shutdown: crate::cancellation::Cancellation,
) -> Result<()> {
    let ServedSite {
        sites,
        initial_config,
        reload_endpoint,
        publications,
    } = served;
    let io = hyper_util::rt::TokioIo::new(stream);
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let sites = sites.clone();
        let initial_config = Arc::clone(&initial_config);
        let reload_endpoint = reload_endpoint.clone();
        let publications = publications.clone();
        async move {
            let req = req.map(|_| ());
            let resp = if req.uri().path() == PREVIEW_ENDPOINT_PATH.as_str() {
                preview::respond(req, peer, sites, publications).await
            } else {
                response::respond(
                    req,
                    sites.installed(),
                    &initial_config,
                    reload_endpoint.as_ref(),
                    sites.unpublished_html(),
                )
            };
            Ok::<_, Infallible>(resp)
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HTTP_HEADER_TIMEOUT);
    let conn = builder.serve_connection(io, service);
    tokio::pin!(conn);
    // Graceful shutdown closes idle keep-alive connections while letting in-flight responses finish.
    tokio::select! {
        completion = &mut conn => completion.map_err(anyhow::Error::from),
        _ = shutdown.cancelled() => Ok(()),
        _ = tokio::time::sleep(idle_lifetime) => {
            conn.as_mut().graceful_shutdown();
            tokio::select! {
                completion = &mut conn => completion.map_err(anyhow::Error::from),
                _ = shutdown.cancelled() => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
    use tola_build::build::{BuildMode, BuildSession};
    use tola_build::config::SiteConfig;

    use super::*;

    const LARGE_ASSET_BYTES: usize = 256 * 1024;
    const SMALL_ASSET_BYTES: usize = 4 * 1024;
    const IDLE_LIFETIME: Duration = Duration::from_millis(250);

    pub(super) use crate::dev::tests::site_config;

    fn served_site() -> (tempfile::TempDir, CurrentSite, Arc<SiteConfig>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("large.bin"), vec![0u8; LARGE_ASSET_BYTES]).unwrap();
        std::fs::write(dir.path().join("small.bin"), vec![0u8; SMALL_ASSET_BYTES]).unwrap();
        let cfg = crate::dev::tests::write_site(
            dir.path(),
            "",
            "[assets]\nfiles = [{ source = \"large.bin\", url = \"/large.bin\" }, { source = \"small.bin\", url = \"/small.bin\" }]\n",
        );
        let mut session = BuildSession::new();
        let (checked, _) = crate::dev::tests::checked_candidate(
            &mut session,
            Arc::clone(&cfg),
            BuildMode::Production,
        );
        let sites = CurrentSite::new();
        sites.replace(&mut session, checked).unwrap();
        (dir, sites, cfg)
    }

    fn serve_client(
        sites: CurrentSite,
        cfg: Arc<SiteConfig>,
    ) -> (DuplexStream, tokio::task::JoinHandle<Result<()>>) {
        let (client, server) = duplex(16 * 1024);
        let serving = tokio::spawn(serve_connection(
            server,
            ([127, 0, 0, 1], 0).into(),
            ServedSite {
                sites,
                initial_config: cfg,
                reload_endpoint: None,
                publications: None,
            },
            IDLE_LIFETIME,
            crate::cancellation::Cancellation::default(),
        ));
        (client, serving)
    }

    async fn request(stream: &mut DuplexStream, path: &str) {
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
    }

    async fn response_head(stream: &mut DuplexStream) -> (Vec<u8>, Vec<u8>) {
        let mut buf = Vec::new();
        let mut unscanned = 0;
        loop {
            if let Some(end) = buf[unscanned..].windows(4).position(|w| w == b"\r\n\r\n") {
                let body = buf.split_off(unscanned + end + 4);
                return (buf, body);
            }
            unscanned = buf.len().saturating_sub(3);
            let mut chunk = [0u8; 16 * 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "connection closed before response head");
            buf.extend_from_slice(&chunk[..read]);
        }
    }

    async fn drain(stream: &mut DuplexStream) -> Vec<u8> {
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        rest
    }

    #[tokio::test]
    async fn in_flight_response_completes() {
        let (_dir, sites, cfg) = served_site();
        let (mut client, serving) = serve_client(sites, cfg);
        request(&mut client, "/large.bin").await;
        let (head, body) = response_head(&mut client).await;
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        assert!(
            head.contains(&format!("content-length: {LARGE_ASSET_BYTES}")),
            "{head}"
        );

        tokio::time::sleep(IDLE_LIFETIME * 3).await;
        assert!(!serving.is_finished());
        let drained = tokio::time::timeout(Duration::from_secs(60), drain(&mut client))
            .await
            .unwrap();
        assert_eq!(body.len() + drained.len(), LARGE_ASSET_BYTES);
        serving.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn idle_connection_closes() {
        let (_dir, sites, cfg) = served_site();
        let (mut client, serving) = serve_client(sites, cfg);
        request(&mut client, "/small.bin").await;
        let (head, mut body) = response_head(&mut client).await;
        assert!(
            String::from_utf8(head)
                .unwrap()
                .starts_with("HTTP/1.1 200 ")
        );
        while body.len() < SMALL_ASSET_BYTES {
            let mut chunk = [0u8; 1024];
            let read = client.read(&mut chunk).await.unwrap();
            assert!(read > 0);
            body.extend_from_slice(&chunk[..read]);
        }

        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(IDLE_LIFETIME * 3, client.read(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(closed, 0);
        serving.await.unwrap().unwrap();
    }

    #[test]
    fn taken_base_port_fallback() {
        let iface = "127.0.0.1".parse().unwrap();
        let held = TcpListener::bind((iface, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();

        let (_listener, addr, fallback) = bind_with_port_fallback(iface, taken).unwrap();
        let fallback = fallback.unwrap();
        assert_eq!(fallback.code, crate::codes::server::PORT_IN_USE);
        assert!(fallback.message.contains(&taken.to_string()));
        assert_ne!(addr.port(), taken);

        let (_listener, _addr, fallback) = bind_with_port_fallback(iface, 0).unwrap();
        assert!(fallback.is_none());
    }
}
