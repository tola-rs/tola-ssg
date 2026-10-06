//! Invocation cancellation and Ctrl+C handling.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tola_build::cancellation::{BuildCancellation, BuildCanceller};

#[derive(Clone, Default)]
pub(crate) struct Cancellation {
    canceller: BuildCanceller,
    notify: CancellationToken,
    interrupted: Arc<AtomicBool>,
}

impl Cancellation {
    pub(crate) fn install() -> Result<Self> {
        let signal = Self::default();
        let shutdown = signal.clone();
        ctrlc::set_handler(move || {
            shutdown.interrupt();
        })
        .map_err(|error| {
            anyhow::Error::new(error).context("Tola could not install its Ctrl+C handler")
        })?;
        Ok(signal)
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.notify.is_cancelled()
    }

    pub(crate) fn token(&self) -> BuildCancellation {
        self.canceller.token()
    }

    pub(crate) fn was_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }

    /// Record user cancellation separately from orderly service shutdown.
    pub(crate) fn interrupt(&self) {
        self.interrupted.store(true, Ordering::Relaxed);
        self.request();
    }

    pub(crate) fn request(&self) {
        self.canceller.cancel();
        self.notify.cancel();
    }

    pub(crate) async fn cancelled(&self) {
        self.notify.cancelled().await;
    }
}

#[cfg(test)]
mod tests {
    use super::Cancellation;

    #[test]
    fn user_interrupt_cancels_workers() {
        let signal = Cancellation::default();
        let worker = signal.token();
        signal.interrupt();
        assert!(signal.was_interrupted());
        assert!(worker.is_cancelled());
    }

    #[tokio::test]
    async fn shutdown_wakes_every_waiter() {
        let signal = Cancellation::default();
        let first = signal.clone();
        let second = signal.clone();
        let worker = signal.token();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(biased; first.cancelled(), second.cancelled(), async {
                signal.request();
            });
        })
        .await
        .expect("shutdown did not wake its waiters");

        assert!(worker.is_cancelled());
        assert!(first.is_requested());
        assert!(second.is_requested());
        assert!(!signal.was_interrupted());
    }

    #[tokio::test]
    async fn late_waiters_see_shutdown() {
        let signal = Cancellation::default();
        signal.request();

        tokio::time::timeout(std::time::Duration::from_secs(2), signal.cancelled())
            .await
            .expect("a late waiter missed shutdown");
    }
}
