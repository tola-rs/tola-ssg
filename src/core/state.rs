//! Build state tracking for serve mode.
//!
//! Three orthogonal states:
//! - `SERVING`: Is the site ready to serve requests? (init phase complete)
//! - `HEALTHY`: Is the build healthy? (can hot-reload vs needs full rebuild)
//! - `SHUTDOWN`: Has shutdown been requested? (Ctrl+C received)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tiny_http::Server;

/// Site is ready to serve requests (init phase complete)
/// - `false`: Return 503
/// - `true`: Serve normally
static SERVING: AtomicBool = AtomicBool::new(false);

/// Build is healthy (has valid cache for hot-reload)
/// - `false`: Next file change triggers full rebuild
/// - `true`: Next file change triggers hot-reload
static HEALTHY: AtomicBool = AtomicBool::new(false);

/// Shutdown has been requested (Ctrl+C received)
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// HTTP server reference for graceful shutdown
static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

/// Shutdown signal sender for actor system
static SHUTDOWN_TX: OnceLock<crossbeam::channel::Sender<()>> = OnceLock::new();

// =============================================================================
// SERVING state
// =============================================================================

/// Check if the site is ready to serve requests
pub fn is_serving() -> bool {
    SERVING.load(Ordering::SeqCst)
}

/// Mark the site as ready to serve (call after initial build completes)
pub fn set_serving() {
    SERVING.store(true, Ordering::SeqCst);
}

// =============================================================================
// HEALTHY state
// =============================================================================

/// Check if the build is healthy (can hot-reload)
pub fn is_healthy() -> bool {
    HEALTHY.load(Ordering::SeqCst)
}

/// Set the health state
pub fn set_healthy(healthy: bool) {
    HEALTHY.store(healthy, Ordering::SeqCst);
}

// =============================================================================
// SHUTDOWN state
// =============================================================================

/// Setup the global Ctrl+C handler. Call once at program start
///
/// The handler behavior depends on whether a server has been registered:
/// - Before `register_server()`: Sets SHUTDOWN flag, process exits naturally
/// - After `register_server()`: Graceful shutdown (unblock server, notify actors)
pub fn setup_shutdown_handler() -> anyhow::Result<()> {
    ctrlc::set_handler(|| {
        SHUTDOWN.store(true, Ordering::SeqCst);

        // Notify actor system
        if let Some(tx) = SHUTDOWN_TX.get() {
            let _ = tx.send(());
        }

        // Unblock HTTP server, or exit immediately if not yet serving
        if let Some(server) = SERVER.get() {
            crate::log!("serve"; "shutting down...");
            server.unblock();
        } else {
            // No server registered yet (e.g., during config prompt)
            // Exit immediately since there's nothing to gracefully shutdown
            std::process::exit(0);
        }
    })
    .map_err(|e| anyhow::anyhow!("failed to set Ctrl+C handler: {}", e))
}

/// Register the HTTP server for graceful shutdown
///
/// Call this after binding the server, before entering the request loop
pub fn register_server(server: Arc<Server>, shutdown_tx: crossbeam::channel::Sender<()>) {
    let _ = SERVER.set(server);
    let _ = SHUTDOWN_TX.set(shutdown_tx);
}

/// Check if shutdown has been requested
///
/// Uses Relaxed ordering for performance - worst case is processing
/// a few more items before stopping, which is acceptable
pub fn is_shutdown() -> bool {
    SHUTDOWN.load(Ordering::Relaxed)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serving() {
        SERVING.store(false, Ordering::SeqCst);
        assert!(!is_serving());

        set_serving();
        assert!(is_serving());
    }

    #[test]
    fn test_healthy() {
        set_healthy(false);
        assert!(!is_healthy());

        set_healthy(true);
        assert!(is_healthy());
    }
}
