//! Development server with live reload support.

mod build;
mod classify;
mod compile;
mod content;
mod lifecycle;
mod path;
mod response;
mod scan;
mod startup;

pub use build::init_serve_build;
pub(crate) use build::start_serve_build;
pub use scan::scan_pages;
pub use startup::serve_with_cache;

use crate::address::SiteIndex;
use crate::{
    config::{SiteConfig, config_handle},
    core::{ContentKind, UrlPath},
    debug, log,
};
use anyhow::Result;
use classify::{ServedOutputKind, classify_served_output};
use crossbeam::channel;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tiny_http::{Request, Server};

/// Default WebSocket port for hot reload
pub const DEFAULT_WS_PORT: u16 = 35729;

/// Actual WebSocket port (may differ from DEFAULT_WS_PORT if port was in use)
/// Updated by coordinator after WebSocket server binds successfully
static ACTUAL_WS_PORT: AtomicU16 = AtomicU16::new(DEFAULT_WS_PORT);

/// Startup scan readiness for progressive serving.
/// Kept in serve module to avoid leaking serve-only state into core globals.
static SCAN_READY: AtomicBool = AtomicBool::new(false);
/// Last observed HTTP request time. Used to keep startup warmup out of the
/// user's way while the first page load is still in flight.
static LAST_REQUEST_MS: AtomicU64 = AtomicU64::new(0);

/// Update the actual WebSocket port (called by coordinator after binding)
pub fn set_actual_ws_port(port: u16) {
    ACTUAL_WS_PORT.store(port, Ordering::Relaxed);
}

/// Get the actual WebSocket port
fn get_actual_ws_port() -> u16 {
    ACTUAL_WS_PORT.load(Ordering::Relaxed)
}

pub(crate) fn set_scan_ready(ready: bool) {
    SCAN_READY.store(ready, Ordering::SeqCst);
}

fn is_scan_ready() -> bool {
    SCAN_READY.load(Ordering::SeqCst)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn note_request_activity() {
    LAST_REQUEST_MS.store(now_millis(), Ordering::SeqCst);
}

pub(crate) fn request_idle_for(duration: Duration) -> bool {
    let last = LAST_REQUEST_MS.load(Ordering::SeqCst);
    last == 0 || now_millis().saturating_sub(last) >= duration.as_millis() as u64
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
    let config = config_handle().current();
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
    pub fn run(self, state: Arc<SiteIndex>) -> Result<()> {
        let handle = config_handle();
        let config = handle.current();
        let actor_handle = lifecycle::spawn_actors(
            handle,
            Arc::clone(&state),
            config.serve.watch,
            self.ws_port,
            self.shutdown_rx,
        );
        run_request_loop(&self.server, state);
        lifecycle::wait_for_shutdown(actor_handle);
        Ok(())
    }
}

fn run_request_loop(server: &Server, state: Arc<SiteIndex>) {
    // Use thread pool to handle requests concurrently
    // This prevents on-demand compilation from blocking other requests
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .expect("failed to create thread pool");

    let config_handle = config_handle();
    for request in server.incoming_requests() {
        let state = Arc::clone(&state);
        pool.spawn(move || {
            let config = config_handle.current();
            if let Err(e) = handle_request(request, &config, state) {
                log!("serve"; "request error: {e}");
            }
        });
    }
}

/// Handle a single HTTP request
fn handle_request(request: Request, config: &SiteConfig, state: Arc<SiteIndex>) -> Result<()> {
    note_request_activity();

    // Early exit if shutdown requested
    if crate::core::is_shutdown() {
        return response::respond_unavailable(request);
    }

    // Serve hotreload.js from memory only when watch mode is enabled.
    // Use actual ws_port which may differ from DEFAULT_WS_PORT after retry.
    let ws_port = config.serve.watch.then_some(get_actual_ws_port());
    if let Some(port) = ws_port {
        use crate::embed::serve::{HOTRELOAD_JS, HotreloadVars};
        let vars = HotreloadVars { ws_port: port };
        if request.url() == HOTRELOAD_JS.url_path_with_vars(&config.build.path_prefix, &vars) {
            return response::respond_hotreload_js(request, port);
        }
    }

    let request_url = request.url().to_string();

    // Serve static output files as early as possible, even during startup scan.
    // This keeps CSS/JS/assets and already-built pages available while the site
    // is still converging.
    if let Some(path) = path::resolve_path(&request_url, &config.build.output) {
        return match classify_served_output(&request_url, &path, config, &state) {
            ServedOutputKind::PageHtml { source } => {
                match compile::compile_on_demand(&source, config, Arc::clone(&state)) {
                    Ok(output_path) => {
                        serve_file_without_recovery(request, &output_path, config, ws_port)
                    }
                    Err(e) => response::respond_compile_error(
                        request,
                        &e,
                        &config.build.path_prefix,
                        ws_port,
                    ),
                }
            }
            ServedOutputKind::NotFoundHtml => response::respond_not_found(request, config, ws_port),
            ServedOutputKind::Asset
            | ServedOutputKind::RedirectHtml
            | ServedOutputKind::GeneratedHtml
            | ServedOutputKind::UnknownHtml => {
                serve_file_with_recovery(request, &request_url, &path, config, state, ws_port)
            }
        };
    }

    if content::is_content_empty(config) {
        return response::respond_welcome(request);
    }

    let serving = crate::core::is_serving();
    let scan_ready = is_scan_ready();
    if !serving && !scan_ready {
        if let Some(source) = guess_source_before_scan(&request_url, config) {
            return match compile::compile_on_demand(&source, config, Arc::clone(&state)) {
                Ok(output_path) => {
                    serve_file_without_recovery(request, &output_path, config, ws_port)
                }
                Err(e) => {
                    response::respond_compile_error(request, &e, &config.build.path_prefix, ws_port)
                }
            };
        }
        return response::respond_loading(request);
    }

    // `HEALTHY` represents hot-reload readiness, not HTTP readiness.
    // While unhealthy (initial/full rebuild or recovery), still allow
    // direct on-demand compilation for requested pages.
    if !crate::core::is_healthy() {
        return serve_unhealthy_request(request, &request_url, config, state, ws_port);
    }

    // On-demand compilation (URL → source → compile → serve from disk)
    let url = crate::core::UrlPath::from_browser(&request_url);
    let source = state.read(|_, address| address.source_for_url(&url));

    if let Some(source) = source {
        return match compile::compile_on_demand(&source, config, state) {
            Ok(output_path) => serve_file_without_recovery(request, &output_path, config, ws_port),
            Err(e) => {
                response::respond_compile_error(request, &e, &config.build.path_prefix, ws_port)
            }
        };
    }

    response::respond_not_found(request, config, ws_port)
}

fn serve_unhealthy_request(
    request: Request,
    request_url: &str,
    config: &SiteConfig,
    state: Arc<SiteIndex>,
    ws_port: Option<u16>,
) -> Result<()> {
    let url = UrlPath::from_browser(request_url);
    let source = state
        .read(|_, address| address.source_for_url(&url))
        .or_else(|| guess_source_before_scan(request_url, config));

    let Some(source) = source else {
        return response::respond_loading(request);
    };

    match compile::compile_on_demand(&source, config, state) {
        Ok(output_path) => serve_file_without_recovery(request, &output_path, config, ws_port),
        Err(e) => response::respond_compile_error(request, &e, &config.build.path_prefix, ws_port),
    }
}

/// Best-effort source lookup before `scan_pages()` has populated AddressSpace.
///
/// This intentionally only supports the default route layout derived from file
/// paths. If multiple candidates map to the same URL, we refuse to guess and
/// keep the loading page until scan metadata is available.
fn guess_source_before_scan(request_url: &str, config: &SiteConfig) -> Option<PathBuf> {
    let url = UrlPath::from_browser(request_url);
    let rel = url.as_str().trim_matches('/');
    let mut matches = Vec::new();

    if rel.is_empty() {
        push_content_candidate(&mut matches, config.build.content.join("index.typ"));
        push_content_candidate(&mut matches, config.build.content.join("index.typst"));
        push_content_candidate(&mut matches, config.build.content.join("index.md"));
        push_content_candidate(&mut matches, config.build.content.join("index.markdown"));
    } else {
        let rel_path = Path::new(rel);
        for ext in ["typ", "typst", "md", "markdown"] {
            push_content_candidate(
                &mut matches,
                config.build.content.join(rel_path).with_extension(ext),
            );
            push_content_candidate(
                &mut matches,
                config
                    .build
                    .content
                    .join(rel_path)
                    .join(format!("index.{ext}")),
            );
        }
    }

    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn push_content_candidate(matches: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_file() && ContentKind::is_content_file(&candidate) {
        matches.push(crate::utils::path::normalize_path(&candidate));
    }
}

fn serve_file_with_recovery(
    request: Request,
    request_url: &str,
    path: &Path,
    config: &SiteConfig,
    state: Arc<SiteIndex>,
    ws_port: Option<u16>,
) -> Result<()> {
    match response::respond_file(request, path, &config.build.path_prefix, ws_port)? {
        response::FileServeResult::Served => Ok(()),
        response::FileServeResult::Missing(request) => {
            debug!(
                "serve";
                "transient missing output for {}, attempting on-demand recovery",
                request_url
            );
            recover_missing_output(request, request_url, config, state, ws_port)
        }
    }
}

fn serve_file_without_recovery(
    request: Request,
    path: &Path,
    config: &SiteConfig,
    ws_port: Option<u16>,
) -> Result<()> {
    match response::respond_file(request, path, &config.build.path_prefix, ws_port)? {
        response::FileServeResult::Served => Ok(()),
        // Single recovery attempt already happened in caller path.
        response::FileServeResult::Missing(request) => response::respond_loading(request),
    }
}

fn recover_missing_output(
    request: Request,
    request_url: &str,
    config: &SiteConfig,
    state: Arc<SiteIndex>,
    ws_port: Option<u16>,
) -> Result<()> {
    let url = crate::core::UrlPath::from_browser(request_url);
    let source = state.read(|_, address| address.source_for_url(&url));

    let Some(source) = source else {
        return response::respond_not_found(request, config, ws_port);
    };

    // Force fresh compile result to avoid serving stale scheduler cache entries.
    crate::compiler::scheduler::SCHEDULER.invalidate(&source);

    match compile::compile_on_demand(&source, config, state) {
        Ok(output_path) => serve_file_without_recovery(request, &output_path, config, ws_port),
        Err(e) => response::respond_compile_error(request, &e, &config.build.path_prefix, ws_port),
    }
}

#[cfg(test)]
mod tests {
    use super::classify::{ServedOutputKind, classify_served_output};
    use super::guess_source_before_scan;
    use crate::address::SiteIndex;
    use crate::config::SiteConfig;
    use crate::core::UrlPath;
    use crate::page::{PageMeta, PageRoute};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn make_test_config(root: &Path) -> SiteConfig {
        let root = crate::utils::path::normalize_path(root);
        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.content = root.join("content");
        config.build.output = root.join("public");
        fs::create_dir_all(&config.build.content).unwrap();
        fs::create_dir_all(&config.build.output).unwrap();
        config
    }

    fn reset_runtime_state(state: &SiteIndex) {
        state.clear();
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn page_route(source: &Path, output: &Path, permalink: &str, is_404: bool) -> PageRoute {
        PageRoute {
            source: source.to_path_buf(),
            is_index: false,
            is_404,
            permalink: UrlPath::from_page(permalink),
            output_file: output.to_path_buf(),
            output_dir: output.parent().unwrap_or(Path::new("")).to_path_buf(),
            full_url: format!("https://example.com{permalink}"),
        }
    }

    #[test]
    fn classify_served_output_returns_page_html_for_canonical_page_output() {
        let state = SiteIndex::new();
        reset_runtime_state(&state);
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let source = config.build.content.join("posts/hello.typ");
        let output = config.build.output.join("posts/hello/index.html");
        write_file(&source, "= Hello");
        write_file(&output, "<html><body>Hello</body></html>");

        state.edit(|_, address| {
            address.register_page(
                page_route(&source, &output, "/posts/hello/", false),
                Some("Hello".to_string()),
            );
        });

        let kind = classify_served_output("/posts/hello/", &output, &config, &state);

        assert!(
            matches!(kind, ServedOutputKind::PageHtml { source: s } if s == crate::utils::path::normalize_path(&source))
        );
    }

    #[test]
    fn classify_served_output_returns_asset_for_non_html_output() {
        let state = SiteIndex::new();
        reset_runtime_state(&state);
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let asset = config.build.output.join("assets/app.css");
        write_file(&asset, "body{}");

        let kind = classify_served_output("/assets/app.css", &asset, &config, &state);

        assert!(matches!(kind, ServedOutputKind::Asset));
    }

    #[test]
    fn classify_served_output_returns_redirect_html_for_alias_output() {
        let state = SiteIndex::new();
        reset_runtime_state(&state);
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let source = config.build.content.join("posts/hello.typ");
        let canonical_output = config.build.output.join("posts/hello/index.html");
        let redirect_output = config.build.output.join("old/index.html");
        write_file(&source, "= Hello");
        write_file(&canonical_output, "<html><body>Hello</body></html>");
        write_file(&redirect_output, "<html><body>Redirect</body></html>");

        state.with_pages(|pages| {
            pages.insert_page(
                UrlPath::from_page("/posts/hello/"),
                PageMeta {
                    aliases: vec!["/old/".to_string()],
                    ..Default::default()
                },
            );
        });

        let kind = classify_served_output("/old/", &redirect_output, &config, &state);

        assert!(matches!(kind, ServedOutputKind::RedirectHtml));
    }

    #[test]
    fn classify_served_output_returns_not_found_html_for_compiled_404_output() {
        let state = SiteIndex::new();
        reset_runtime_state(&state);
        let dir = TempDir::new().unwrap();
        let mut config = make_test_config(dir.path());
        config.site.not_found = Some(PathBuf::from("content/404.typ"));
        let source = config.build.content.join("404.typ");
        let output = config.build.output.join("404.html");
        write_file(&source, "= Not Found");
        write_file(&output, "<html><body>404</body></html>");

        state.edit(|_, address| {
            address.register_page(
                page_route(&source, &output, "/404.html/", true),
                Some("404".to_string()),
            );
        });

        let kind = classify_served_output("/404.html", &output, &config, &state);

        assert!(matches!(kind, ServedOutputKind::NotFoundHtml));
    }

    #[test]
    fn guess_source_before_scan_finds_root_index() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let source = config.build.content.join("index.md");
        fs::write(&source, "# Home").unwrap();

        let guessed = guess_source_before_scan("/", &config).expect("root source");
        assert_eq!(guessed, crate::utils::path::normalize_path(&source));
    }

    #[test]
    fn guess_source_before_scan_finds_unique_default_route() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let source = config.build.content.join("posts").join("hello.typ");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "= Hello").unwrap();

        let guessed = guess_source_before_scan("/posts/hello/", &config).expect("post source");
        assert_eq!(guessed, crate::utils::path::normalize_path(&source));
    }

    #[test]
    fn guess_source_before_scan_refuses_ambiguous_route() {
        let dir = TempDir::new().unwrap();
        let config = make_test_config(dir.path());
        let direct = config.build.content.join("posts.typ");
        let nested = config.build.content.join("posts").join("index.md");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&direct, "= Direct").unwrap();
        fs::write(&nested, "# Nested").unwrap();

        assert!(guess_source_before_scan("/posts/", &config).is_none());
    }
}
