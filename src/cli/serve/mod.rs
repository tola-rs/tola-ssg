//! Development server with live reload support.

mod build;
mod compile;
mod content;
mod lifecycle;
mod path;
mod response;
mod scan;

pub use build::{init_serve_build, serve_build};
pub use scan::scan_pages;

use crate::{
    config::{SiteConfig, cfg},
    debug, log,
};
use anyhow::Result;
use crossbeam::channel;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tiny_http::{Request, Server};

/// Default WebSocket port for hot reload
pub const DEFAULT_WS_PORT: u16 = 35729;

/// Actual WebSocket port (may differ from DEFAULT_WS_PORT if port was in use)
/// Updated by coordinator after WebSocket server binds successfully
static ACTUAL_WS_PORT: AtomicU16 = AtomicU16::new(DEFAULT_WS_PORT);

/// Update the actual WebSocket port (called by coordinator after binding)
pub fn set_actual_ws_port(port: u16) {
    ACTUAL_WS_PORT.store(port, Ordering::Relaxed);
}

/// Get the actual WebSocket port
fn get_actual_ws_port() -> u16 {
    ACTUAL_WS_PORT.load(Ordering::Relaxed)
}

/// Bound server ready to accept requests
pub struct BoundServer {
    server: Arc<Server>,
    addr: SocketAddr,
    ws_port: Option<u16>,
    shutdown_rx: channel::Receiver<()>,
}

/// Bind the HTTP server without starting the request loop
///
/// This allows the caller to start background tasks (like scan) before
/// entering the request loop, while still being able to respond to requests
/// with a 503 response
pub fn bind_server() -> Result<BoundServer> {
    let config = cfg();
    let (server, addr) = lifecycle::bind_with_retry(config.serve.interface, config.serve.port)?;
    let server = Arc::new(server);

    let ws_port = config.serve.watch.then_some(DEFAULT_WS_PORT);
    if ws_port.is_some() {
        debug!("hotreload"; "ws://localhost:{}", DEFAULT_WS_PORT);
    }

    let (shutdown_tx, shutdown_rx) = channel::unbounded::<()>();
    lifecycle::register_server_for_shutdown(Arc::clone(&server), shutdown_tx);

    log!("serve"; "http://{}", addr);

    Ok(BoundServer {
        server,
        addr,
        ws_port,
        shutdown_rx,
    })
}

impl BoundServer {
    /// Get the bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Start the request loop (blocking).
    pub fn run(self) -> Result<()> {
        let config = cfg();
        let actor_handle = lifecycle::spawn_actors(
            Arc::clone(&config),
            config.serve.watch,
            self.ws_port,
            self.shutdown_rx,
        );
        run_request_loop(&self.server);
        lifecycle::wait_for_shutdown(actor_handle);
        Ok(())
    }
}

fn run_request_loop(server: &Server) {
    let config = cfg();
    // Use thread pool to handle requests concurrently
    // This prevents on-demand compilation from blocking other requests
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("failed to create thread pool");

    for request in server.incoming_requests() {
        let config = Arc::clone(&config);
        pool.spawn(move || {
            if let Err(e) = handle_request(request, &config) {
                log!("serve"; "request error: {e}");
            }
        });
    }
}

/// Handle a single HTTP request
fn handle_request(request: Request, config: &SiteConfig) -> Result<()> {
    // Early exit if shutdown requested
    if crate::core::is_shutdown() {
        return response::respond_unavailable(request);
    }

    // Serve hotreload.js from memory (doesn't depend on file system)
    // Use actual ws_port which may differ from DEFAULT_WS_PORT after retry
    let ws_port = Some(get_actual_ws_port());
    if let Some(port) = ws_port {
        use crate::embed::serve::{HOTRELOAD_JS, HotreloadVars};
        let vars = HotreloadVars { ws_port: port };
        if request.url() == HOTRELOAD_JS.url_path_with_vars(&vars) {
            return response::respond_hotreload_js(request, port);
        }
    }

    if !crate::core::is_serving() {
        return response::respond_loading(request);
    }

    if content::is_content_empty(config) {
        return response::respond_welcome(request);
    }

    // Try to serve from disk (already compiled)
    if let Some(path) = path::resolve_path(request.url(), &config.build.output) {
        return response::respond_file(request, &path, ws_port);
    }

    // On-demand compilation (URL → source → compile → serve from disk)
    let url = crate::core::UrlPath::from_browser(request.url());
    let source = crate::core::GLOBAL_ADDRESS_SPACE
        .read()
        .source_for_url(&url);

    if let Some(source) = source {
        return match compile::compile_on_demand(&source, config) {
            Ok(output_path) => response::respond_file(request, &output_path, ws_port),
            Err(e) => response::respond_compile_error(request, &e, ws_port),
        };
    }

    response::respond_not_found(request, config, ws_port)
}
