//! HTTP response handlers.

use super::content::maybe_inject_hotreload;
use crate::config::SiteConfig;
use anyhow::{Context, Result};
use std::{fs, path::Path};
use tiny_http::{Header, Method, Request, Response, StatusCode};

/// Respond with a static file, optionally injecting hotreload script.
pub fn respond_file(request: Request, path: &Path, ws_port: Option<u16>) -> Result<()> {
    let content_type = crate::utils::mime::from_path(path);

    if is_head_request(&request) {
        return send_head(request, 200, content_type);
    }

    // Check for Range header (video/audio seeking)
    if let Some(range) = get_range_header(&request) {
        return respond_range(request, path, content_type, &range);
    }

    let body = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let body = maybe_inject_hotreload(body, content_type, ws_port);

    send_body(request, 200, content_type, body)
}

/// Handle Range request for media files (video/audio seeking).
fn respond_range(
    request: Request,
    path: &Path,
    content_type: &'static str,
    range: &str,
) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let file_size = fs::metadata(path)?.len();

    // Parse "bytes=start-end" format
    let range = range.strip_prefix("bytes=").unwrap_or(range);
    let (start, end) = parse_range(range, file_size)?;

    let length = end - start + 1;

    // Stream the requested range - no memory allocation for large ranges
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let reader = file.take(length);

    // Build 206 Partial Content response with streaming reader
    let content_range = format!("bytes {}-{}/{}", start, end, file_size);
    let response = Response::new(
        StatusCode(206),
        vec![
            Header::from_bytes("Content-Type", content_type).unwrap(),
            Header::from_bytes("Content-Range", content_range.as_bytes()).unwrap(),
            Header::from_bytes("Accept-Ranges", "bytes").unwrap(),
        ],
        reader,
        Some(length as usize),
        None,
    );

    request.respond(response)?;
    Ok(())
}

/// Parse Range header value "start-end" into (start, end) bytes.
fn parse_range(range: &str, file_size: u64) -> Result<(u64, u64)> {
    let range = range.trim();
    let parts: Vec<&str> = range.split('-').collect();

    let (start, end) = match parts.as_slice() {
        // "0-499" - specific range
        [s, e] if !s.is_empty() && !e.is_empty() => {
            let start: u64 = s.trim().parse().unwrap_or(0);
            let end: u64 = e.trim().parse().unwrap_or(file_size - 1);
            (start, end.min(file_size - 1))
        }
        // "0-" - from start to end
        [s, ""] if !s.is_empty() => {
            let start: u64 = s.trim().parse().unwrap_or(0);
            (start, file_size - 1)
        }
        // "-500" - last 500 bytes
        ["", e] if !e.is_empty() => {
            let suffix: u64 = e.trim().parse().unwrap_or(0);
            let start = file_size.saturating_sub(suffix);
            (start, file_size - 1)
        }
        _ => (0, file_size - 1),
    };

    Ok((start, end))
}

/// Extract Range header from request.
fn get_range_header(request: &Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("range"))
        .map(|h| h.value.to_string())
}

/// Respond with 404 page (custom or default).
pub fn respond_not_found(
    request: Request,
    config: &SiteConfig,
    ws_port: Option<u16>,
) -> Result<()> {
    use crate::utils::mime::types::{HTML, PLAIN};

    let custom_404 = config.build.output.join("404.html");
    let has_custom = custom_404.is_file();

    if is_head_request(&request) {
        let mime = if has_custom { HTML } else { PLAIN };
        return send_head(request, 404, mime);
    }

    if has_custom
        && let Ok(body) = fs::read(&custom_404)
    {
        let body = maybe_inject_hotreload(body, HTML, ws_port);
        return send_body(request, 404, HTML, body);
    }

    send_body(request, 404, PLAIN, b"404 Not Found".to_vec())
}

/// Respond with loading page (build not ready).
pub fn respond_loading(request: Request) -> Result<()> {
    let body = crate::embed::serve::LOADING_HTML.to_string();
    send_html(request, body)
}

/// Respond with 503 Service Unavailable (server shutting down).
pub fn respond_unavailable(request: Request) -> Result<()> {
    use crate::utils::mime::types::PLAIN;
    send_body(request, 503, PLAIN, b"503 Service Unavailable".to_vec())
}

/// Respond with welcome page (empty content directory).
///
/// Includes a polling script that auto-refreshes when content is created.
/// Note: HEAD requests return without X-Tola-Ready to prevent infinite refresh loop.
pub fn respond_welcome(request: Request) -> Result<()> {
    use crate::embed::serve::{WelcomeVars, WELCOME_HTML};
    use crate::utils::mime::types::HTML;

    // HEAD request: return without X-Tola-Ready (polling checks this header)
    if is_head_request(&request) {
        let response = Response::empty(StatusCode(200))
            .with_header(make_header("Content-Type", HTML));
        return request.respond(response).map_err(Into::into);
    }

    let body = WELCOME_HTML.render(&WelcomeVars {
        title: "Welcome",
        version: env!("CARGO_PKG_VERSION"),
    });

    // Inject polling script to auto-refresh when content is created
    // Note: fetch ignores fragment, but we need to preserve it for navigation
    // Check both X-Tola-Ready header AND status 200 (not 404)
    let poll_script = r#"<script>
(function(){
    var url = location.origin + location.pathname + location.search;
    var poll = function() {
        fetch(url, { method: 'HEAD' })
            .then(function(r) {
                if (r.ok && r.headers.get('X-Tola-Ready') === 'true') location.reload();
            })
            .catch(function() {});
    };
    poll();
    setInterval(poll, 1000);
})();
</script>"#;

    let body = body.replace("</body>", &format!("{poll_script}</body>"));
    send_html(request, body)
}

fn is_head_request(request: &Request) -> bool {
    request.method() == &Method::Head
}

fn send_head(request: Request, status: u16, content_type: &'static str) -> Result<()> {
    let response = Response::empty(StatusCode(status))
        .with_header(make_header("Content-Type", content_type))
        .with_header(make_header("X-Tola-Ready", "true"));
    request.respond(response)?;
    Ok(())
}

fn send_body(
    request: Request,
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
) -> Result<()> {
    let response = Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(make_header("Content-Type", content_type))
        .with_header(make_header("X-Tola-Ready", "true"));
    request.respond(response)?;
    Ok(())
}

/// Send HTML without X-Tola-Ready (for loading/welcome pages).
fn send_html(request: Request, body: String) -> Result<()> {
    use crate::utils::mime::types::HTML;
    let response = Response::from_string(body).with_header(make_header("Content-Type", HTML));
    request.respond(response)?;
    Ok(())
}

/// Respond with compilation error (500), with hotreload for auto-refresh.
pub fn respond_compile_error(
    request: Request,
    error: &anyhow::Error,
    ws_port: Option<u16>,
) -> Result<()> {
    use crate::utils::mime::types::HTML;

    let error_str = format!("{error:#}");
    let msg = crate::utils::html::escape(&error_str);
    let body = format!(
        "<html><body><h1>Compilation Error</h1><pre>{msg}</pre></body></html>",
    );
    let body = maybe_inject_hotreload(body.into_bytes(), HTML, ws_port);
    send_body(request, 500, HTML, body)
}

/// Respond with hotreload.js from memory.
pub fn respond_hotreload_js(request: Request, ws_port: u16) -> Result<()> {
    use crate::embed::serve::{HotreloadVars, HOTRELOAD_JS};
    use crate::utils::mime::types::JAVASCRIPT;

    let vars = HotreloadVars { ws_port };
    let body = HOTRELOAD_JS.render(&vars);
    send_body(request, 200, JAVASCRIPT, body.into_bytes())
}

fn make_header(key: &'static str, value: &'static str) -> Header {
    Header::from_bytes(key, value).unwrap()
}
