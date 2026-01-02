//! Server lifecycle management.

use crate::{actor::Coordinator, config::SiteConfig, core::register_server, log};
use anyhow::Result;
use crossbeam::channel::{Receiver, Sender};
use std::{
    net::SocketAddr,
    sync::Arc,
    thread::{self, JoinHandle},
};
use tiny_http::Server;

/// Maximum number of port binding attempts.
const MAX_PORT_RETRIES: u16 = 10;

/// Bind to the specified interface and port, with automatic port retry.
pub fn bind_with_retry(
    interface: std::net::IpAddr,
    base_port: u16,
) -> Result<(Server, SocketAddr)> {
    for offset in 0..MAX_PORT_RETRIES {
        let port = base_port.saturating_add(offset);
        let addr = SocketAddr::new(interface, port);

        match Server::http(addr) {
            Ok(server) => {
                if offset > 0 {
                    log!("serve"; "port {} in use, using {} instead", base_port, port);
                }
                return Ok((server, addr));
            }
            Err(_) if offset + 1 < MAX_PORT_RETRIES => continue,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to bind after {} attempts (ports {}-{}): {}",
                    MAX_PORT_RETRIES,
                    base_port,
                    port,
                    e
                ));
            }
        }
    }
    unreachable!()
}

/// Register server for graceful shutdown.
///
/// This registers the server with the global shutdown handler set up in main().
/// When Ctrl+C is pressed, the handler will unblock the server and notify actors.
pub fn register_server_for_shutdown(server: Arc<Server>, shutdown_tx: Sender<()>) {
    register_server(server, shutdown_tx);
}

/// Spawn the actor system for file watching and hot reload.
pub fn spawn_actors(
    config: Arc<SiteConfig>,
    watch_enabled: bool,
    ws_port: Option<u16>,
    shutdown_rx: Receiver<()>,
) -> Option<JoinHandle<()>> {
    if !watch_enabled {
        return None;
    }

    Some(thread::spawn(move || {
        run_actor_system(config, ws_port, shutdown_rx);
    }))
}

fn run_actor_system(config: Arc<SiteConfig>, ws_port: Option<u16>, shutdown_rx: Receiver<()>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    rt.block_on(async {
        let mut coordinator = Coordinator::with_config(config).with_shutdown_signal(shutdown_rx);
        if let Some(port) = ws_port {
            coordinator = coordinator.with_ws_port(port);
        }
        if let Err(e) = coordinator.run().await {
            log!("actor"; "error: {}", e);
        }
    });
}

/// Wait for actor system to shutdown gracefully (max 2 seconds).
pub fn wait_for_shutdown(handle: Option<JoinHandle<()>>) {
    let Some(handle) = handle else { return };

    for _ in 0..40 {
        if handle.is_finished() {
            let _ = handle.join();
            return;
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
}
