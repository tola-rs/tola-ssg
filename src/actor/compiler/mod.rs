//! Compiler Actor - Typst Compilation Wrapper
//!
//! Handles file compilation with priority-based scheduling:
//! - Direct/Active files: compiled immediately for instant feedback
//! - Affected files: compiled in background, interruptible by new requests

mod dispatch;
mod handlers;
mod pipeline;
mod tasks;
mod utils;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::messages::{CompilerMsg, VdomMsg};
use crate::config::SiteConfig;
use crate::reload::compile::CompileOutcome;

pub(super) struct BatchResult {
    pub(super) outcomes: Vec<CompileOutcome>,
    pub(super) pages_hash: u64,
}

pub(super) type BackgroundTask = JoinHandle<BatchResult>;
pub(super) const ACTIVE_RECOMPILE_COOLDOWN: Duration = Duration::from_millis(250);

pub struct CompilerActor {
    pub(super) rx: mpsc::Receiver<CompilerMsg>,
    pub(super) vdom_tx: mpsc::Sender<VdomMsg>,
    pub(super) config: Arc<SiteConfig>,
    pub(super) last_active_recompile: Option<Instant>,
}

impl CompilerActor {
    pub fn new(
        rx: mpsc::Receiver<CompilerMsg>,
        vdom_tx: mpsc::Sender<VdomMsg>,
        config: Arc<SiteConfig>,
    ) -> Self {
        Self {
            rx,
            vdom_tx,
            config,
            last_active_recompile: None,
        }
    }
}
