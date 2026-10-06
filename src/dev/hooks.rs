//! Serial after-publish commands with one latest pending development revision.

use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow};
use tola_build::cancellation::{BuildCancellation, BuildCancelled, BuildCanceller};
use tola_build::diagnostic::Diagnostic;
use tola_build::site::SiteRevision;

#[derive(Clone)]
pub(crate) struct HookSender {
    mailbox: Arc<HookMailbox>,
    cancellation: BuildCancellation,
    shutdown: BuildCancellation,
}

/// One published revision waiting for its after-publish commands.
///
/// The round that published it travels alongside, so a command's records name the round and the
/// revision they belong to even when the command outlives that round.
struct PendingRevision {
    /// The development round that published the revision; a session that numbers no rounds
    /// leaves it unnamed.
    round: Option<u64>,
    site: Arc<SiteRevision>,
}

enum PendingHooks {
    Waiting,
    Revision(PendingRevision),
    Closed,
}

struct HookMailbox {
    pending: Mutex<PendingHooks>,
    available: Condvar,
}

impl HookMailbox {
    fn close(&self) {
        let discarded = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *pending, PendingHooks::Closed)
        };
        self.available.notify_all();
        drop(discarded);
    }
}

struct HookReceiver {
    mailbox: Arc<HookMailbox>,
}

impl HookReceiver {
    fn next(
        &self,
        cancellation: &BuildCancellation,
        shutdown: &BuildCancellation,
    ) -> Option<PendingRevision> {
        let mut pending = self
            .mailbox
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if cancellation.is_cancelled() || shutdown.is_cancelled() {
                return None;
            }
            match &*pending {
                PendingHooks::Closed => return None,
                PendingHooks::Waiting => {
                    pending = self
                        .mailbox
                        .available
                        .wait(pending)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                PendingHooks::Revision(_) => {
                    let PendingHooks::Revision(pending) =
                        std::mem::replace(&mut *pending, PendingHooks::Waiting)
                    else {
                        unreachable!("the locked mailbox contains a pending revision");
                    };
                    return Some(pending);
                }
            }
        }
    }
}

impl Drop for HookReceiver {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

impl HookSender {
    /// Queue one published revision for its after-publish commands.
    ///
    /// `round` is the development round that published it, or `None` for a session that numbers
    /// no rounds.
    pub(crate) fn enqueue(&self, site: Arc<SiteRevision>, round: Option<u64>) -> Result<()> {
        if self.cancellation.is_cancelled()
            || self.shutdown.is_cancelled()
            || !tola_build::hooks::has_after_publish_hooks(site.config(), site.mode())
        {
            return Ok(());
        }
        let discarded = {
            let mut pending = self
                .mailbox
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
                return Ok(());
            }
            if matches!(*pending, PendingHooks::Closed) {
                return Err(anyhow!(
                    "Tola could no longer run after-publish commands; restart `tola dev`"
                ));
            }
            std::mem::replace(
                &mut *pending,
                PendingHooks::Revision(PendingRevision { round, site }),
            )
        };
        self.mailbox.available.notify_one();
        drop(discarded);
        Ok(())
    }
}

/// The running revision completes while newer builds replace the one pending
/// revision. Shutdown cancels the running hook and discards pending work.
pub(crate) struct HookQueue {
    sender: HookSender,
    shutdown: BuildCanceller,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HookQueue {
    pub(crate) fn start(
        output: crate::cli::output::CommandOutput,
        browser: Option<crate::dev::reload::transport::ReloadDiagnostics>,
        cancellation: BuildCancellation,
    ) -> Result<Self> {
        let mailbox = Arc::new(HookMailbox {
            pending: Mutex::new(PendingHooks::Waiting),
            available: Condvar::new(),
        });
        let pending = HookReceiver {
            mailbox: Arc::clone(&mailbox),
        };
        let shutdown = BuildCanceller::new();
        let worker_shutdown = shutdown.token();
        let worker_cancellation = cancellation.clone();
        let worker = std::thread::Builder::new()
            .name("tola-after-publish".into())
            .spawn(move || {
                while let Some(pending) = pending.next(&worker_cancellation, &worker_shutdown) {
                    if let Err(error) = consume_revision(&pending, &worker_shutdown) {
                        if worker_cancellation.is_cancelled()
                            || worker_shutdown.is_cancelled()
                            || error.chain().any(|cause| cause.is::<BuildCancelled>())
                        {
                            break;
                        }
                        tracing::debug!(
                            target: "tola::dev",
                            revision = pending.site.manifest().revision().as_str(),
                            %error,
                            "after-publish command failed"
                        );
                        let diagnostic = after_publish_failure(&error);
                        let _ = output.status("Site built; an after-publish command failed");
                        let _ = output.diagnostic(&diagnostic);
                        if let Some(browser) = &browser {
                            browser.after_publish_failed(&pending.site, &diagnostic);
                        }
                    }
                }
            })
            .context("Tola could not start its after-publish worker; restart `tola dev`")?;
        Ok(Self {
            sender: HookSender {
                mailbox,
                cancellation,
                shutdown: shutdown.token(),
            },
            shutdown,
            worker: Some(worker),
        })
    }

    pub(crate) fn sender(&self) -> HookSender {
        self.sender.clone()
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        self.close()
    }

    fn close(&mut self) -> Result<()> {
        self.shutdown.cancel();
        self.sender.mailbox.close();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("Tola could not stop its after-publish worker"))?;
        }
        Ok(())
    }
}

impl Drop for HookQueue {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            tracing::error!(target: "tola::dev", %error, "after-publish shutdown failed");
        }
    }
}

/// Run one revision's after-publish commands under its published identity.
///
/// The span is entered for the synchronous command run, so a command's hook events name their
/// round and revision even while a later round publishes.
fn consume_revision(pending: &PendingRevision, cancellation: &BuildCancellation) -> Result<()> {
    let site = pending.site.as_ref();
    let span = tracing::info_span!(
        target: "tola::dev",
        "after-publish",
        round = pending.round,
        revision = site.manifest().revision().as_str(),
    );
    span.in_scope(|| {
        let files = site.hook_output(cancellation)?;
        tola_build::hooks::run_after_publish_hooks(
            site.config(),
            site.mode(),
            files.root(),
            cancellation,
        )
    })
}

fn after_publish_failure(error: &anyhow::Error) -> Diagnostic {
    tola_build::diagnostic::attached(error)
        .and_then(|diagnostics| diagnostics.first().cloned())
        .unwrap_or_else(|| {
            tola_build::diagnostic::fallback(crate::codes::hook::AFTER_PUBLISH, error)
        })
        .with_help(
            "Fix the failing command in `build.hooks.after-publish`, then save a file to rebuild",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn queue_shutdown_stays_local() {
        let output = crate::cli::output::CommandOutput::new(
            crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
            None,
        );
        let cancellation = BuildCancellation::default();
        let queue = HookQueue::start(output, None, cancellation.clone()).unwrap();
        let sender = queue.sender();
        let (completed, completion) = mpsc::channel();
        let closing = std::thread::spawn(move || completed.send(queue.finish()).unwrap());

        let closed = completion.recv_timeout(std::time::Duration::from_secs(2));
        drop(sender);
        closing.join().unwrap();

        closed
            .expect("closing waited for a retained sender")
            .unwrap();
        assert!(!cancellation.is_cancelled());
    }
}
