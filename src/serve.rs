//! Development server with live reload support.
//!
//! Serves the built site and watches for file changes if enabled.

use crate::{config::SiteConfig, log, watch::watch_for_changes_blocking};
use anyhow::{Context, Result};
use axum::{
    Router,
    http::{StatusCode, Uri},
    response::{Html, IntoResponse},
};
use std::{
    fs,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;
use tower_http::services::ServeDir;

/// Directory listing HTML template
const DIRECTORY_TEMPLATE: &str = include_str!("../assets/serve/directory.html");

/// Welcome page HTML template
const WELCOME_TEMPLATE: &str = include_str!("../assets/serve/welcome.html");

/// Start the development server with file watching
pub async fn serve_site(config: &'static SiteConfig) -> Result<()> {
    let server_ready = Arc::new(AtomicBool::new(false));

    // Spawn server task
    tokio::spawn({
        let server_ready = Arc::clone(&server_ready);
        async move {
            if let Err(err) = start_server(config, server_ready).await {
                log!("serve"; "{err}");
            }
        }
    });

    // Spawn file watcher thread
    std::thread::spawn({
        let server_ready = Arc::clone(&server_ready);
        move || {
            wait_for_server(true, &server_ready);
            if let Err(err) = watch_for_changes_blocking(config, server_ready) {
                log!("watch"; "{err}");
            }
        }
    });

    tokio::signal::ctrl_c().await.ok();
    wait_for_server(false, &server_ready);

    Ok(())
}

/// Block until server reaches the expected ready state
fn wait_for_server(ready: bool, server_ready: &Arc<AtomicBool>) {
    let state = if ready { "start" } else { "quit" };
    log!("watch"; "Waiting for server to {state}");
    while ready != server_ready.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Start the HTTP server on configured address
pub async fn start_server(
    config: &'static SiteConfig,
    server_ready: Arc<AtomicBool>,
) -> Result<()> {
    let addr = SocketAddr::new(
        IpAddr::from_str(&config.serve.interface)?,
        config.serve.port,
    );

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind to address {addr}"))?;

    let app = create_router(config);

    server_ready.store(true, Ordering::Release);
    log!("serve"; "serving site on http://{}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(server_ready))
        .await
        .context("[serve] failed to start")?;

    Ok(())
}

/// Create the Axum router with static file serving
fn create_router(config: &'static SiteConfig) -> Router {
    let serve_root = config.build.output.clone();
    let serve_dir = ServeDir::new(&config.build.output)
        .append_index_html_on_directories(false)
        .not_found_service(axum::routing::get(move |uri| {
            let root = serve_root.clone();
            async move { handle_path(uri, root).await }
        }));
    Router::new().fallback_service(serve_dir)
}

/// Handle incoming requests, serving files or directory listings
async fn handle_path(uri: Uri, serve_root: PathBuf) -> impl IntoResponse {
    let request_path = uri.path().trim_matches('/');
    let request_path = urlencoding::decode(request_path)
        .map(|s| s.into_owned())
        .unwrap_or_default();
    let local_path = serve_root.join(&request_path);

    // Try to read the file directly
    if local_path.is_file()
        && let Ok(content) = fs::read_to_string(&local_path)
    {
        return Html(content).into_response();
    }

    // If it's a directory, try to serve index.html or generate listing
    if local_path.is_dir() {
        let index_path = local_path.join("index.html");
        if let Ok(content) = fs::read_to_string(&index_path) {
            return Html(content).into_response();
        }

        if let Ok(listing) = generate_directory_listing(&local_path, &request_path) {
            return Html(listing).into_response();
        }
    }

    // Fallback to 404
    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}

/// Generate HTML directory listing for browsing
fn generate_directory_listing(dir_path: &PathBuf, request_path: &str) -> std::io::Result<String> {
    let entries: Vec<_> = fs::read_dir(dir_path)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            // Filter out hidden files (starting with '.')
            !entry.file_name().to_string_lossy().starts_with('.')
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

    Ok(DIRECTORY_TEMPLATE
        .replace("{path}", request_path)
        .replace("{parent_link}", &parent_link)
        .replace("{entries}", &entries.join("\n            ")))
}

/// Handle graceful shutdown on Ctrl+C
async fn shutdown_signal(server_ready: Arc<AtomicBool>) {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C signal handler");
    server_ready.store(false, Ordering::Release);
    log!("serve"; "shutting down gracefully...");
}
