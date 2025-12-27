//! Development server with live reload support.
//!
//! This module provides a lightweight HTTP server for local development,
//! built on `tiny_http` with the following features:
//!
//! - Static file serving from the build output directory
//! - Automatic `index.html` resolution for directories
//! - Directory listing with a clean HTML interface
//! - File watching and auto-rebuild (via `watch` module)
//! - Graceful shutdown on Ctrl+C
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────┐
//! │   Main Thread   │     │  Watcher Thread  │
//! │  (HTTP Server)  │     │  (File Monitor)  │
//! └────────┬────────┘     └────────┬─────────┘
//!          │                       │
//!          ▼                       ▼
//!    Handle requests         Detect changes
//!    Serve files             Trigger rebuild
//! └─────────────────────────────────────────────┘
//!                    │
//!                    ▼
//!            config.build.output
//!              (public/ dir)
//! ```

use crate::{config::{cfg, SiteConfig}, log, watch::watch_for_changes_blocking};
use anyhow::{Context, Result};
use std::{
    fs,
    io::Cursor,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tiny_http::{Header, Request, Response, Server, StatusCode};

// ============================================================================
// Constants - HTML Templates
// ============================================================================

/// Directory listing HTML template (embedded at compile time)
const DIRECTORY_TEMPLATE: &str = include_str!("embed/serve/directory.html");

/// Welcome page HTML template (shown when output directory is empty)
const WELCOME_TEMPLATE: &str = include_str!("embed/serve/welcome.html");

// ============================================================================
// Server Entry Point
// ============================================================================

/// Start the development server with optional file watching.
///
/// This function:
/// 1. Binds to the configured interface and port
/// 2. Sets up Ctrl+C handler for graceful shutdown
/// 3. Spawns file watcher thread (if enabled)
/// 4. Enters the main request handling loop
///
/// The server blocks until Ctrl+C is received.
pub fn serve_site() -> Result<()> {
    let c = cfg();
    let addr = SocketAddr::new(c.serve.interface.parse()?, c.serve.port);

    let server =
        Arc::new(Server::http(addr).map_err(|e| anyhow::anyhow!("Failed to bind to {addr}: {e}"))?);

    // Set up Ctrl+C handler for graceful shutdown
    let server_for_signal = Arc::clone(&server);
    ctrlc::set_handler(move || {
        log!("serve"; "shutting down...");
        server_for_signal.unblock();
    })
    .context("Failed to set Ctrl+C handler")?;

    log!("serve"; "http://{}", addr);

    // Spawn file watcher thread
    if c.serve.watch {
        std::thread::spawn(move || {
            if let Err(err) = watch_for_changes_blocking() {
                log!("watch"; "{err}");
            }
        });
    }

    // Handle requests in main thread (blocks until Ctrl+C)
    for request in server.incoming_requests() {
        // Re-load config on each request to pick up hot-reloaded changes
        if let Err(e) = handle_request(request, &cfg()) {
            log!("serve"; "request error: {e}");
        }
    }

    Ok(())
}

// ============================================================================
// Request Handling
// ============================================================================

/// Handle a single HTTP request.
///
/// Request resolution order:
/// 1. Exact file match → serve file
/// 2. Directory with index.html → serve index.html
/// 3. Directory without index.html → generate listing
/// 4. Nothing found → 404
fn handle_request(request: Request, config: &SiteConfig) -> Result<()> {
    let serve_root = &config.build.output;
    let data_dir_name = config.build.data.to_string_lossy();

    // Decode URL-encoded characters (e.g., %20 → space)
    let url_path = urlencoding::decode(request.url())
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_default();

    // Strip query string (e.g., ?t=123456) before resolving path
    // This is important for cache-busting URLs like "font.woff2?t=123"
    let path_without_query = url_path.split('?').next().unwrap_or(&url_path);
    let request_path = path_without_query.trim_matches('/');
    let local_path = serve_root.join(request_path);

    // Try to serve the file directly
    if local_path.is_file() {
        return serve_file(request, &local_path);
    }

    // If it's a directory, try index.html or generate listing
    if local_path.is_dir() {
        let index_path = local_path.join("index.html");
        if index_path.is_file() {
            return serve_file(request, &index_path);
        }

        if let Ok(listing) = generate_directory_listing(&local_path, request_path, &data_dir_name) {
            return serve_html(request, listing);
        }
    }

    // 404 Not Found
    serve_not_found(request)
}

// ============================================================================
// Response Helpers
// ============================================================================

/// Serve a file with appropriate content type.
fn serve_file(request: Request, path: &Path) -> Result<()> {
    let content = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let content_type = guess_content_type(path);

    let response = Response::from_data(content)
        .with_header(Header::from_bytes("Content-Type", content_type).unwrap());

    request.respond(response)?;
    Ok(())
}

/// Serve HTML content.
fn serve_html(request: Request, content: String) -> Result<()> {
    let response = Response::from_string(content)
        .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap());
    request.respond(response)?;
    Ok(())
}

/// Serve 404 Not Found response.
fn serve_not_found(request: Request) -> Result<()> {
    let response = Response::new(
        StatusCode(404),
        vec![Header::from_bytes("Content-Type", "text/plain").unwrap()],
        Cursor::new("404 Not Found"),
        Some(13),
        None,
    );
    request.respond(response)?;
    Ok(())
}

// ============================================================================
// Content Type Detection
// ============================================================================

/// Guess MIME content type from file extension.
///
/// Returns `application/octet-stream` for unknown extensions.
fn guess_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        // Web content
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",

        // Images
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",

        // Fonts
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",

        // Documents
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",

        // Default binary
        _ => "application/octet-stream",
    }
}

// ============================================================================
// Directory Listing
// ============================================================================

/// Generate HTML directory listing for browsing.
///
/// Features:
/// - Only shows directories and `.html` files
/// - Filters out hidden files (starting with '.')
/// - Filters out internal data directory
/// - Shows folder/file icons
/// - Provides parent directory navigation
/// - Falls back to welcome page if directory is empty
fn generate_directory_listing(dir_path: &PathBuf, request_path: &str, data_dir_name: &str) -> std::io::Result<String> {
    let entries: Vec<_> = fs::read_dir(dir_path)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Filter out hidden files (starting with '.') and internal data directory
            let is_hidden = name_str.starts_with('.') || name_str == data_dir_name;

            // Allow directories
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

            // Only show .html files, filter out feed.xml, sitemap.xml, etc.
            !is_hidden && (is_dir || name_str.ends_with(".html"))
        })
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let icon = if is_dir { "📁" } else { "📄" };
            let href = if request_path.is_empty() {
                format!("/{name}")
            } else {
                format!("/{request_path}/{name}")
            };
            format!(r#"<li><span class="icon">{icon}</span><a href="{href}">{name}</a></li>"#)
        })
        .collect();

    // If no visible entries, show welcome page
    if entries.is_empty() {
        return Ok(WELCOME_TEMPLATE
            .replace("{title}", "Welcome")
            .replace("{version}", env!("CARGO_PKG_VERSION")));
    }

    // Generate parent link if not at root
    let parent_link = if request_path.is_empty() {
        String::new()
    } else {
        let parent_path = std::path::Path::new(request_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent_href = if parent_path.is_empty() {
            "/".to_string()
        } else {
            format!("/{parent_path}")
        };
        format!(
            r#"<li class="parent"><span class="icon">📂</span><a href="{parent_href}">..</a></li>"#
        )
    };

    #[allow(clippy::literal_string_with_formatting_args)] // These are template placeholders, not format args
    Ok(DIRECTORY_TEMPLATE
        .replace("{path}", request_path)
        .replace("{parent_link}", &parent_link)
        .replace("{entries}", &entries.join("\n            ")))
}
