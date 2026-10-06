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

use crate::dev::site::CurrentSite;

pub(super) use preview::PublicationRequest;

const MAX_PORT_RETRIES: u16 = 10;
const MAX_HTTP_CONNECTIONS: usize = 256;
const HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a connection with no response in flight is retained before it is recycled.
const HTTP_IDLE_CONNECTION_LIFETIME: Duration = Duration::from_secs(60);

/// Bind a nonblocking listener, trying consecutive ports when needed.
pub(super) fn bind_with_port_fallback(
    interface: std::net::IpAddr,
    base_port: u16,
    output: &crate::cli::output::CommandOutput,
) -> Result<(TcpListener, SocketAddr)> {
    for offset in 0..MAX_PORT_RETRIES {
        let port = base_port.saturating_add(offset);
        let address = SocketAddr::new(interface, port);
        let bound = TcpListener::bind(address).and_then(|listener| {
            listener.set_nonblocking(true)?;
            let address = listener.local_addr()?;
            Ok((listener, address))
        });
        match bound {
            Ok((listener, address)) => {
                if offset > 0 {
                    output.diagnostic(
                        &tola_build::diagnostic::Diagnostic::new(
                            crate::codes::server::PORT_IN_USE,
                            tola_build::diagnostic::Severity::Warning,
                            format!("port `{base_port}` is already in use"),
                        )
                        .with_note(format!("serving http://{address} instead"))
                        .with_help(format!("Use `--port {}` to keep this port", address.port())),
                    )?;
                }
                return Ok((listener, address));
            }
            Err(_) if offset + 1 < MAX_PORT_RETRIES => {}
            Err(_) => {
                let message = format!("Tola could not start the development server on {interface}");
                let diagnostic = tola_build::diagnostic::Diagnostic::new(
                    crate::codes::command::FAILED,
                    tola_build::diagnostic::Severity::Error,
                    message.clone(),
                )
                .with_help("Choose another address or port with `--interface`/`--port`");
                return Err(tola_build::diagnostic::DiagnosticError::new(
                    message,
                    vec![diagnostic],
                )
                .into());
            }
        }
    }
    unreachable!()
}

pub(super) async fn serve_http(
    listener: tokio::net::TcpListener,
    sites: CurrentSite,
    initial_config: Arc<tola_build::config::ResolvedSiteConfig>,
    reload_endpoint: Option<crate::dev::reload::transport::ReloadEndpoint>,
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
    shutdown: crate::cancellation::Cancellation,
) -> Result<()> {
    let mut connections = tokio::task::JoinSet::new();
    let connection_limit = Arc::new(tokio::sync::Semaphore::new(MAX_HTTP_CONNECTIONS));
    let serving = async {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                connection = listener.accept() => {
                    let (stream, address) = match connection {
                        Ok(connection) => connection,
                        Err(error) => {
                            tracing::debug!(target: "tola::dev", %error, "accept failed");
                            return Err(anyhow!(
                                "Tola could not accept a development request; restart `tola dev`"
                            ));
                        }
                    };
                    let Ok(connection_permit) = Arc::clone(&connection_limit).try_acquire_owned()
                    else {
                        tracing::debug!(target: "tola::dev", %address, limit = MAX_HTTP_CONNECTIONS, "HTTP connection limit reached");
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
                        let _connection_permit = connection_permit;
                        if let Err(error) = serve_connection(
                            stream,
                            address,
                            served,
                            HTTP_IDLE_CONNECTION_LIFETIME,
                            shutdown.clone(),
                        )
                        .await
                            && !shutdown.is_requested()
                        {
                            tracing::debug!(target: "tola::dev", %error, "connection error");
                        }
                    });
                }
                Some(completion) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = completion {
                        tracing::debug!(target: "tola::dev", %error, "connection task failed");
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

/// The one request path the development server answers under `RESERVED_ROOT`.
///
/// The address is a cross-language contract: the VS Code client and the preview end-to-end suite
/// spell the same URL, so a change here needs those call sites in the same change. A site route
/// cannot take the path over: the reserved namespace is refused for a site mount and for a
/// published output.
static PREVIEW_ENDPOINT_PATH: LazyLock<String> =
    LazyLock::new(|| format!("/{RESERVED_ROOT}/{PREVIEW_ENDPOINT}"));

/// The current revision and endpoints one development connection answers its requests from.
struct ServedSite {
    sites: CurrentSite,
    initial_config: Arc<tola_build::config::ResolvedSiteConfig>,
    reload_endpoint: Option<crate::dev::reload::transport::ReloadEndpoint>,
    publications: Option<tokio::sync::mpsc::Sender<PublicationRequest>>,
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
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
    let service = hyper::service::service_fn(move |request: Request<Incoming>| {
        let sites = sites.clone();
        let initial_config = Arc::clone(&initial_config);
        let reload_endpoint = reload_endpoint.clone();
        let publications = publications.clone();
        async move {
            let request = request.map(|_| ());
            let reply = if request.uri().path() == PREVIEW_ENDPOINT_PATH.as_str() {
                preview::respond(request, peer, sites, &initial_config, publications).await
            } else {
                response::respond(
                    request,
                    sites.installed(),
                    &initial_config,
                    reload_endpoint.as_ref(),
                    sites.unpublished_html(),
                )
            };
            Ok::<_, Infallible>(reply)
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(HTTP_HEADER_TIMEOUT);
    let connection = builder.serve_connection(io, service);
    tokio::pin!(connection);
    // The idle lifetime ends a connection that outlived it: hyper's graceful shutdown closes an
    // idle keep-alive connection at once and, during an exchange, lets the response in flight
    // finish before closing, so the deadline never truncates a response. Nothing bounds a
    // response itself — any wall clock would cut off a client that is merely slow — and
    // `MAX_HTTP_CONNECTIONS` caps how many connections such a client can hold.
    tokio::select! {
        completion = &mut connection => completion.map_err(anyhow::Error::from),
        _ = shutdown.cancelled() => Ok(()),
        _ = tokio::time::sleep(idle_lifetime) => {
            connection.as_mut().graceful_shutdown();
            tokio::select! {
                completion = &mut connection => completion.map_err(anyhow::Error::from),
                _ = shutdown.cancelled() => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tola_build::build::{BuildMode, BuildRequest, BuildSession};
    use tola_build::config::ResolvedSiteConfig;

    use super::*;

    /// Far above what the socket buffers hold, so a client that stops reading leaves the response
    /// in flight.
    const LARGE_ASSET_BYTES: usize = 32 * 1024 * 1024;
    const SMALL_ASSET_BYTES: usize = 4 * 1024;
    const IDLE_LIFETIME: Duration = Duration::from_millis(250);

    pub(super) fn site_config(root: &std::path::Path, configuration: &str) -> ResolvedSiteConfig {
        tola_build::config::loading::resolve_site_config(
            &root.join("tola.toml"),
            configuration,
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    /// One published revision with `large.bin` and `small.bin`.
    fn served_site() -> (tempfile::TempDir, CurrentSite, Arc<ResolvedSiteConfig>) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("content")).unwrap();
        std::fs::write(root.path().join("site.typ"), "").unwrap();
        std::fs::write(root.path().join("large.bin"), vec![0u8; LARGE_ASSET_BYTES]).unwrap();
        std::fs::write(root.path().join("small.bin"), vec![0u8; SMALL_ASSET_BYTES]).unwrap();
        let config = Arc::new(site_config(
            root.path(),
            "[assets]\nfiles = [{ source = \"large.bin\", url = \"/large.bin\" }, { source = \"small.bin\", url = \"/small.bin\" }]\n",
        ));
        let mut session = BuildSession::new();
        let attempt = session.prepare(
            Arc::clone(&config),
            BuildRequest::new(BuildMode::Production),
        );
        let candidate = attempt.run().unwrap().into_unchecked_revision(None);
        let checked = candidate
            .check(&tola_build::cancellation::BuildCancellation::new())
            .unwrap()
            .into_checked()
            .unwrap();
        let sites = CurrentSite::new();
        sites.replace(&mut session, checked).unwrap();
        (root, sites, config)
    }

    /// Serve one loopback connection, returning the client end and the serving task.
    async fn serve_client(
        sites: CurrentSite,
        config: Arc<ResolvedSiteConfig>,
    ) -> (tokio::net::TcpStream, tokio::task::JoinHandle<Result<()>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, peer) = listener.accept().await.unwrap();
        let serving = tokio::spawn(serve_connection(
            server,
            peer,
            ServedSite {
                sites,
                initial_config: config,
                reload_endpoint: None,
                publications: None,
            },
            IDLE_LIFETIME,
            crate::cancellation::Cancellation::default(),
        ));
        (client, serving)
    }

    async fn request(stream: &mut tokio::net::TcpStream, path: &str) {
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
    }

    /// The response head and the body bytes that arrived with it.
    async fn response_head(stream: &mut tokio::net::TcpStream) -> (Vec<u8>, Vec<u8>) {
        let mut received = Vec::new();
        let mut unscanned = 0;
        loop {
            if let Some(terminator) = received[unscanned..]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
            {
                let body = received.split_off(unscanned + terminator + 4);
                return (received, body);
            }
            unscanned = received.len().saturating_sub(3);
            let mut chunk = [0u8; 16 * 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "the connection closed before the response head");
            received.extend_from_slice(&chunk[..read]);
        }
    }

    /// The bytes a client receives from `stream` until the server closes it.
    async fn drain(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        rest
    }

    #[tokio::test]
    async fn response_outliving_connection_lifetime_arrives_complete() {
        let (_root, sites, config) = served_site();
        let (mut client, serving) = serve_client(sites, config).await;
        request(&mut client, "/large.bin").await;
        let (head, body) = response_head(&mut client).await;
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
        assert!(
            head.contains(&format!("content-length: {LARGE_ASSET_BYTES}")),
            "{head}"
        );

        // Only the client is missing here: the server is blocked writing a body larger than the
        // socket buffers, so the lifetime deadline arrives mid-response. A finished task here
        // would mean the kernel buffered the whole asset and the deadline missed the response.
        tokio::time::sleep(IDLE_LIFETIME * 3).await;
        assert!(
            !serving.is_finished(),
            "the deadline must arrive while the response is in flight"
        );
        let drained = tokio::time::timeout(Duration::from_secs(60), drain(&mut client))
            .await
            .expect("the response completes after the deadline");
        assert_eq!(body.len() + drained.len(), LARGE_ASSET_BYTES);
        serving.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn idle_connection_closes_at_connection_lifetime() {
        let (_root, sites, config) = served_site();
        let (mut client, serving) = serve_client(sites, config).await;
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
            assert!(read > 0, "the small asset arrived complete");
            body.extend_from_slice(&chunk[..read]);
        }

        // The exchange is complete and the connection waits for another request.
        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(IDLE_LIFETIME * 3, client.read(&mut byte))
            .await
            .expect("the idle connection ends at its lifetime")
            .unwrap();
        assert_eq!(closed, 0, "the server closed the idle connection");
        serving.await.unwrap().unwrap();
    }
}
