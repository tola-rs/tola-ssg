//! Actor Coordinator - Wires up the Hot Reload Actor System
//!
//! # Responsibility
//!
//! The Coordinator is a **thin orchestrator** that:
//! - Creates communication channels
//! - Wires up actors
//! - Runs them concurrently
//!
//! It does NOT contain business logic - that lives in `pipeline/`.
//!
//! # Architecture
//!
//! ```text
//! FsActor --> CompilerActor --> VdomActor --> WsActor
//!    |              |              |            |
//!    +--------------+--------------+------------+
//!                 Linear Message Flow
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossbeam::channel::Receiver;
use tokio::sync::mpsc;

use super::compiler::CompilerActor;
use super::fs::FsActor;
use super::messages::{CompilerMsg, VdomMsg, WsMsg};
use super::vdom::VdomActor;
use super::ws::WsActor;
use crate::config::SiteConfig;

/// Channel buffer size
const CHANNEL_BUFFER: usize = 32;

/// Coordinator - wires up and runs the actor system
pub struct Coordinator {
    config: Arc<SiteConfig>,
    ws_port: Option<u16>,
    /// Optional shutdown signal receiver
    shutdown_rx: Option<Receiver<()>>,
}

impl Coordinator {
    /// Create from Arc<SiteConfig>
    pub fn with_config(config: Arc<SiteConfig>) -> Self {
        Self {
            config,
            ws_port: None,
            shutdown_rx: None,
        }
    }

    /// Set WebSocket port
    pub fn with_ws_port(mut self, port: u16) -> Self {
        self.ws_port = Some(port);
        self
    }

    /// Set shutdown signal receiver
    pub fn with_shutdown_signal(mut self, rx: Receiver<()>) -> Self {
        self.shutdown_rx = Some(rx);
        self
    }

    /// Run the actor system
    pub async fn run(mut self) -> Result<()> {
        // Create channels
        let (compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(CHANNEL_BUFFER);
        let (vdom_tx, vdom_rx) = mpsc::channel::<VdomMsg>(CHANNEL_BUFFER);
        let (ws_tx, ws_rx) = mpsc::channel::<WsMsg>(CHANNEL_BUFFER);

        // Start WebSocket server
        if let Some(port) = self.ws_port {
            match crate::reload::server::start_ws_server_with_channel(port, ws_tx.clone()) {
                Ok(actual_port) => {
                    crate::cli::serve::set_actual_ws_port(actual_port);
                }
                Err(e) => {
                    crate::log!("actor"; "websocket server failed: {}", e);
                }
            }
        }

        // Create actors - VdomActor tries to restore cache and diagnostics first
        let watch_paths = self.watch_paths();
        let fs_actor = FsActor::new(watch_paths, compiler_tx.clone(), self.config.clone())
            .map_err(|e| anyhow::anyhow!("watcher failed: {}", e))?;

        let compiler_actor = CompilerActor::new(compiler_rx, vdom_tx.clone(), self.config.clone());
        let (vdom_actor, restored_count, first_error, restored_warnings) =
            VdomActor::new(vdom_rx, ws_tx.clone(), self.config.get_root().to_path_buf());

        // Create WsActor with initial pending error if restored
        let ws_actor = match first_error {
            Some((path, error)) => WsActor::new(ws_rx).with_pending_error(path, error),
            None => WsActor::new(ws_rx),
        };
        crate::debug!("vdom"; "cache: {} entries", restored_count);

        // Display restored warnings
        if !restored_warnings.is_empty() {
            let max = self
                .config
                .build
                .diagnostics
                .max_warnings
                .unwrap_or(usize::MAX);
            for warning in restored_warnings.iter().take(max) {
                eprintln!("{}", warning);
            }
            let remaining = restored_warnings.len().saturating_sub(max);
            if remaining > 0 {
                eprintln!("... and {} more warnings", remaining);
            }
        }

        // Run actors until shutdown signal
        crate::debug!("actor"; "start");
        let shutdown_rx = self.shutdown_rx.take();
        let _ = run_actors(
            fs_actor,
            compiler_actor,
            vdom_actor,
            ws_actor,
            vdom_tx,
            shutdown_rx,
        )
        .await;

        crate::debug!("actor"; "stopped");
        Ok(())
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        let root = self.config.get_root();
        let mut paths = vec![root.join(&self.config.build.content)];
        for dep in &self.config.build.deps {
            paths.push(root.join(dep));
        }
        // Watch asset directories (nested) - only if exists
        for source in self.config.build.assets.nested_sources() {
            if source.exists() {
                paths.push(source.to_path_buf());
            }
        }
        // Watch asset files (flatten) - watch parent directories if exists
        for source in self.config.build.assets.flatten_sources() {
            if let Some(parent) = source.parent() {
                let parent_buf = parent.to_path_buf();
                if parent.exists() && !paths.contains(&parent_buf) {
                    paths.push(parent_buf);
                }
            }
        }
        // Watch config file for full rebuild trigger
        if self.config.config_path.exists() {
            paths.push(self.config.config_path.clone());
        }
        // Watch output directory for hook-generated artifact changes.
        // Output events are classified separately to avoid rebuild loops.
        let output_dir = self.config.paths().output_dir();
        let _ = std::fs::create_dir_all(&output_dir);
        if !paths.contains(&output_dir) {
            paths.push(output_dir);
        }
        paths
    }
}

/// Run all actors concurrently
async fn run_actors(
    fs: FsActor,
    compiler: CompilerActor,
    vdom: VdomActor,
    ws: WsActor,
    vdom_tx: mpsc::Sender<VdomMsg>,
    shutdown_rx: Option<Receiver<()>>,
) -> Result<()> {
    // Spawn VdomActor and keep its handle so we can wait for it to finish
    let vdom_handle = tokio::spawn(async move { vdom.run().await });

    // Spawn other actors
    let fs_handle = tokio::spawn(async move { fs.run().await });
    let compiler_handle = tokio::spawn(async move { compiler.run().await });
    let ws_handle = tokio::spawn(async move { ws.run().await });

    // Wait for shutdown signal (poll-based since std::sync::mpsc)
    if let Some(rx) = shutdown_rx {
        loop {
            // Check for shutdown signal
            if rx.try_recv().is_ok() {
                crate::debug!("actor"; "shutdown signal received");
                break;
            }
            // Small sleep to avoid busy-waiting
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    } else {
        // No shutdown signal, just wait for any actor to complete
        tokio::select! {
            _ = fs_handle => {}
            _ = compiler_handle => {}
            _ = ws_handle => {}
        }
    }

    // Send shutdown to VdomActor so it can persist
    crate::debug!("actor"; "sending shutdown to vdom");
    let _ = vdom_tx.send(VdomMsg::Shutdown).await;

    // Wait for VdomActor to complete persist
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), vdom_handle).await;

    Ok(())
}
