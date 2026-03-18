//! HTTP response handlers.

use super::content::maybe_inject_hotreload;
use crate::config::SiteConfig;
use anyhow::{Context, Result};
use std::{fs, path::Path};
use tiny_http::{Header, Method, Request, Response, StatusCode};

/// Result of attempting to serve a file from disk.
///
/// `Missing` indicates the file disappeared before read and may be recovered
/// by recompiling from source.
pub enum FileServeResult {
    Served,
    Missing(Request),
}

/// Respond with a static file, optionally injecting hotreload script
pub fn respond_file(
    request: Request,
    path: &Path,
    ws_port: Option<u16>,
) -> Result<FileServeResult> {
    let content_type = crate::utils::mime::from_path(path);
    let no_cache = content_type == crate::utils::mime::types::HTML;

    if is_head_request(&request) {
        send_head(request, 200, content_type, no_cache)?;
        return Ok(FileServeResult::Served);
    }

    // Check for Range header (video/audio seeking)
    if let Some(range) = get_range_header(&request) {
        respond_range(request, path, content_type, &range)?;
        return Ok(FileServeResult::Served);
    }

    let body = match fs::read(path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileServeResult::Missing(request));
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", path.display()));
        }
    };
    let body = maybe_inject_hotreload(body, content_type, ws_port);

    send_body(request, 200, content_type, body, no_cache)?;
    Ok(FileServeResult::Served)
}

/// Handle Range request for media files (video/audio seeking)
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

/// Parse Range header value "start-end" into (start, end) bytes
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

/// Extract Range header from request
fn get_range_header(request: &Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("range"))
        .map(|h| h.value.to_string())
}

/// Respond with 404 page (custom or default)
///
/// For HTML 404 pages, reads directly from source for hot reload support
/// For compiled 404 pages (typst), reads from output directory
pub fn respond_not_found(
    request: Request,
    config: &SiteConfig,
    ws_port: Option<u16>,
) -> Result<()> {
    use crate::utils::mime::types::{HTML, PLAIN};

    // Try to find 404 page: source HTML or compiled output
    let (body, found) = if let Some(not_found) = &config.site.not_found {
        // Check if it's an HTML file (read from source for hot reload)
        if not_found.extension().and_then(|e| e.to_str()) == Some("html") {
            let source = config.root_join(not_found);
            if source.is_file() {
                (fs::read(&source).ok(), true)
            } else {
                (None, false)
            }
        } else {
            // Compiled file (typst) - read from output
            let output = config.build.output.join("404.html");
            (fs::read(&output).ok(), output.is_file())
        }
    } else {
        (None, false)
    };

    if is_head_request(&request) {
        let mime = if found { HTML } else { PLAIN };
        return send_head(request, 404, mime, false);
    }

    if let Some(body) = body {
        let body = maybe_inject_hotreload(body, HTML, ws_port);
        return send_body(request, 404, HTML, body, false);
    }

    send_body(request, 404, PLAIN, b"404 Not Found".to_vec(), false)
}

/// Respond with 503 + auto-retry (build not ready yet)
pub fn respond_loading(request: Request) -> Result<()> {
    use crate::utils::mime::types::HTML;

    // HEAD requests are used by polling logic; keep response lightweight.
    if is_head_request(&request) {
        let response =
            Response::empty(StatusCode(503)).with_header(make_header("Content-Type", HTML));
        request.respond(response)?;
        return Ok(());
    }

    // Keep a stable loading page and poll readiness via HEAD.
    // This avoids repeated full-page meta-refresh flicker during startup.
    let body = r#"<!doctype html>
<html><body>Loading...
<script>
(function() {
  var url = location.origin + location.pathname + location.search;
  var poll = function() {
    fetch(url, { method: 'HEAD' })
      .then(function(r) {
        if (r.headers.get('X-Tola-Ready') === 'true') {
          location.reload();
        }
      })
      .catch(function() {});
  };
  poll();
  setInterval(poll, 500);
})();
</script>
</body></html>"#;

    let response = Response::from_string(body)
        .with_status_code(StatusCode(503))
        .with_header(make_header("Content-Type", HTML));
    request.respond(response)?;
    Ok(())
}

/// Respond with 503 Service Unavailable (server shutting down)
pub fn respond_unavailable(request: Request) -> Result<()> {
    use crate::utils::mime::types::PLAIN;
    send_body(
        request,
        503,
        PLAIN,
        b"503 Service Unavailable".to_vec(),
        false,
    )
}

/// Respond with welcome page (empty content directory)
///
/// Includes a polling script that auto-refreshes when content is created
/// Note: HEAD requests return without X-Tola-Ready to prevent infinite refresh loop
pub fn respond_welcome(request: Request) -> Result<()> {
    use crate::embed::serve::{WELCOME_HTML, WelcomeVars};
    use crate::utils::mime::types::HTML;

    // HEAD request: return without X-Tola-Ready (polling checks this header)
    if is_head_request(&request) {
        let response =
            Response::empty(StatusCode(200)).with_header(make_header("Content-Type", HTML));
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

fn send_head(
    request: Request,
    status: u16,
    content_type: &'static str,
    no_cache: bool,
) -> Result<()> {
    let response = Response::empty(StatusCode(status))
        .with_header(make_header("Content-Type", content_type))
        .with_header(make_header("X-Tola-Ready", "true"));
    let response = if no_cache {
        with_no_cache_headers(response)
    } else {
        response
    };
    request.respond(response)?;
    Ok(())
}

fn send_body(
    request: Request,
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    no_cache: bool,
) -> Result<()> {
    let body = if content_type.starts_with("text/html") {
        crate::utils::html::ensure_doctype_bytes(body)
    } else {
        body
    };

    let response = Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(make_header("Content-Type", content_type))
        .with_header(make_header("X-Tola-Ready", "true"));
    let response = if no_cache {
        with_no_cache_headers(response)
    } else {
        response
    };
    request.respond(response)?;
    Ok(())
}
/// Send HTML without X-Tola-Ready (for welcome pages)
fn send_html(request: Request, body: String) -> Result<()> {
    use crate::utils::mime::types::HTML;
    let response = Response::from_string(crate::utils::html::ensure_doctype(body))
        .with_header(make_header("Content-Type", HTML));
    request.respond(response)?;
    Ok(())
}

/// Respond with compilation error (500), with hotreload for auto-refresh
pub fn respond_compile_error(
    request: Request,
    error: &anyhow::Error,
    ws_port: Option<u16>,
) -> Result<()> {
    use crate::utils::mime::types::HTML;

    let error_str = format!("{error:#}");
    let msg = crate::utils::html::escape(&error_str);
    let body = format!("<html><body><h1>Compilation Error</h1><pre>{msg}</pre></body></html>",);
    let body = maybe_inject_hotreload(body.into_bytes(), HTML, ws_port);
    send_body(request, 500, HTML, body, false)
}

/// Respond with hotreload.js from memory
pub fn respond_hotreload_js(request: Request, ws_port: u16) -> Result<()> {
    use crate::embed::serve::{HOTRELOAD_JS, HotreloadVars};
    use crate::utils::mime::types::JAVASCRIPT;

    let vars = HotreloadVars { ws_port };
    let body = HOTRELOAD_JS.render(&vars);
    send_body(request, 200, JAVASCRIPT, body.into_bytes(), false)
}

fn make_header(key: &'static str, value: &'static str) -> Header {
    Header::from_bytes(key, value).unwrap()
}

fn with_no_cache_headers<R: std::io::Read>(response: Response<R>) -> Response<R> {
    response
        .with_header(make_header(
            "Cache-Control",
            "no-store, no-cache, must-revalidate, max-age=0",
        ))
        .with_header(make_header("Pragma", "no-cache"))
        .with_header(make_header("Expires", "0"))
}

#[cfg(test)]
mod tests {

    #[test]
    fn prepends_doctype_to_html_bytes() {
        let body =
            crate::utils::html::ensure_doctype_bytes(b"<html><body>Hi</body></html>".to_vec());
        assert!(
            String::from_utf8(body)
                .unwrap()
                .starts_with("<!DOCTYPE html>\n<html>")
        );
    }

    #[test]
    fn does_not_duplicate_existing_doctype() {
        let body = crate::utils::html::ensure_doctype("<!DOCTYPE html>\n<html></html>".to_string());
        assert_eq!(body.matches("<!DOCTYPE html>").count(), 1);
    }
}
