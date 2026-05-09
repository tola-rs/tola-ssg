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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::messages::{CompilerMsg, VdomMsg};
use crate::address::SiteIndex;
use crate::compiler::page::{PageStateEpoch, TypstHost};
use crate::config::{ConfigHandle, SiteConfig};
use crate::reload::compile::CompileOutcome;

pub(super) struct BatchResult {
    pub(super) config: Arc<SiteConfig>,
    pub(super) outcomes: Vec<CompileOutcome>,
    pub(super) pages_hash: u64,
    pub(super) watched_post_paths: Option<Vec<PathBuf>>,
}

pub(super) type BackgroundTask = JoinHandle<BatchResult>;
pub(super) const ACTIVE_RECOMPILE_COOLDOWN: Duration = Duration::from_millis(250);

pub(super) struct CachedTypstHost {
    pub(super) config: Arc<SiteConfig>,
    pub(super) host: Arc<TypstHost>,
}

pub struct CompilerActor {
    pub(super) rx: mpsc::Receiver<CompilerMsg>,
    pub(super) vdom_tx: mpsc::Sender<VdomMsg>,
    pub(super) config: ConfigHandle,
    pub(super) state: Arc<SiteIndex>,
    pub(super) last_active_recompile: Option<Instant>,
    pub(super) page_epoch: PageStateEpoch,
    pub(super) typst_host: Option<CachedTypstHost>,
}

impl CompilerActor {
    pub fn new(
        rx: mpsc::Receiver<CompilerMsg>,
        vdom_tx: mpsc::Sender<VdomMsg>,
        config: ConfigHandle,
        state: Arc<SiteIndex>,
    ) -> Self {
        Self {
            rx,
            vdom_tx,
            config,
            state,
            last_active_recompile: None,
            page_epoch: PageStateEpoch::new(),
            typst_host: None,
        }
    }

    pub(super) fn current_config_and_typst_host(&mut self) -> (Arc<SiteConfig>, Arc<TypstHost>) {
        let config = self.config.current();
        if let Some(cached) = &self.typst_host
            && Arc::ptr_eq(&cached.config, &config)
        {
            return (config, Arc::clone(&cached.host));
        }

        let host = Arc::new(TypstHost::for_config(&config));
        self.typst_host = Some(CachedTypstHost {
            config: Arc::clone(&config),
            host: Arc::clone(&host),
        });
        (config, host)
    }
}
