//! Actor Coordinator - Wires up the Hot Reload Actor System
//!
//! The Coordinator is a thin orchestrator that:
//! - Creates communication channels
//! - Wires up actors
//! - Runs them concurrently

mod runtime;
mod watch_paths;

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

const CHANNEL_BUFFER: usize = 32;

/// Coordinator - wires up and runs the actor system.
pub struct Coordinator {
    config: Arc<SiteConfig>,
    ws_port: Option<u16>,
    shutdown_rx: Option<Receiver<()>>,
}

impl Coordinator {
    /// Create from Arc<SiteConfig>.
    pub fn with_config(config: Arc<SiteConfig>) -> Self {
        Self {
            config,
            ws_port: None,
            shutdown_rx: None,
        }
    }

    /// Set WebSocket port.
    pub fn with_ws_port(mut self, port: u16) -> Self {
        self.ws_port = Some(port);
        self
    }

    /// Set shutdown signal receiver.
    pub fn with_shutdown_signal(mut self, rx: Receiver<()>) -> Self {
        self.shutdown_rx = Some(rx);
        self
    }

    /// Run the actor system.
    pub async fn run(mut self) -> Result<()> {
        let (compiler_tx, compiler_rx) = mpsc::channel::<CompilerMsg>(CHANNEL_BUFFER);
        let (vdom_tx, vdom_rx) = mpsc::channel::<VdomMsg>(CHANNEL_BUFFER);
        let (ws_tx, ws_rx) = mpsc::channel::<WsMsg>(CHANNEL_BUFFER);

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

        let watch_paths = watch_paths::collect_watch_paths(&self.config);
        let fs_actor = FsActor::new(watch_paths, compiler_tx.clone(), self.config.clone())
            .map_err(|e| anyhow::anyhow!("watcher failed: {}", e))?;

        let compiler_actor = CompilerActor::new(compiler_rx, vdom_tx.clone(), self.config.clone());
        let (vdom_actor, restored_count, first_error, restored_warnings) =
            VdomActor::new(vdom_rx, ws_tx.clone(), self.config.get_root().to_path_buf());

        let ws_actor = match first_error {
            Some((path, error)) => WsActor::new(ws_rx).with_pending_error(path, error),
            None => WsActor::new(ws_rx),
        };
        crate::debug!("vdom"; "cache: {} entries", restored_count);

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

        crate::debug!("actor"; "start");
        let shutdown_rx = self.shutdown_rx.take();
        let _ = runtime::run_actors(
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
}
