//! Serial after-publish commands with one latest pending development revision.

use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow};
use tola_build::cancellation::{BuildCancellation, BuildCancelled, BuildCanceller};
use tola_build::site::SiteRevision;
use tola_diagnostic::Diagnostic;

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
    worker: Option<std::thread::JoinHandle<Result<()>>>,
}

impl HookQueue {
    pub(crate) fn start(
        output: crate::cli::output::CommandOutput,
        browser: Option<crate::dev::reload::transport::ReloadDiagnostics>,
        cancellation: crate::cancellation::Cancellation,
        events: super::events::Events,
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
        let worker_token = worker_cancellation.token();
        let worker = std::thread::Builder::new()
            .name("tola-after-publish".into())
            .spawn(move || {
                let mut completed: Option<CompletedRun> = None;
                while let Some(pending) = pending.next(&worker_token, &worker_shutdown) {
                    let revision = pending.site.manifest().revision().as_str();
                    match pending.site.after_publish_dependencies() {
                        Ok(dependencies)
                            if completed
                                .as_ref()
                                .is_some_and(|run| run.dependencies == dependencies) =>
                        {
                            tracing::debug!(
                                target: "tola::dev",
                                revision,
                                "kept the previous after-publish outcome for unchanged dependencies"
                            );
                            if let Some(diagnostics) =
                                completed.as_ref().and_then(|run| run.failure.as_ref())
                            {
                                let _ =
                                    output.status("Site built; an after-publish command failed");
                                let _ = output.diagnostics(diagnostics);
                                if let (Some(browser), Some(diagnostic)) =
                                    (&browser, diagnostics.first())
                                {
                                    browser.after_publish_failed(&pending.site, diagnostic);
                                }
                            }
                        }
                        Ok(dependencies) => {
                            let outcome = match consume_revision(&pending, &worker_shutdown, events)
                            {
                                Ok(outcome) => outcome,
                                Err(error) => {
                                    worker_cancellation.request();
                                    return Err(error);
                                }
                            };
                            let failure = match outcome {
                                Ok(diagnostics) => {
                                    let _ = output.diagnostics(&diagnostics);
                                    None
                                }
                                Err(error) => {
                                    if worker_cancellation.is_requested()
                                        || worker_shutdown.is_cancelled()
                                        || error.chain().any(|cause| cause.is::<BuildCancelled>())
                                    {
                                        break;
                                    }
                                    tracing::debug!(
                                        target: "tola::dev",
                                        revision,
                                        %error,
                                        "after-publish command failed"
                                    );
                                    let diagnostic = after_publish_failure(&error);
                                    let diagnostics = crate::cli::output::attached_or_fallback(
                                        &error,
                                        crate::codes::hook::AFTER_PUBLISH,
                                    );
                                    let _ = output
                                        .status("Site built; an after-publish command failed");
                                    let _ = output.diagnostics(&diagnostics);
                                    if let Some(browser) = &browser {
                                        browser.after_publish_failed(&pending.site, &diagnostic);
                                    }
                                    Some(diagnostics)
                                }
                            };
                            completed = Some(CompletedRun {
                                dependencies,
                                failure,
                            });
                        }
                        Err(error) => {
                            completed = None;
                            let diagnostic = after_publish_failure(&error);
                            let diagnostics = crate::cli::output::attached_or_fallback(
                                &error,
                                crate::codes::hook::AFTER_PUBLISH,
                            );
                            let _ = output.status("Site built; an after-publish command failed");
                            let _ = output.diagnostics(&diagnostics);
                            if let Some(browser) = &browser {
                                browser.after_publish_failed(&pending.site, &diagnostic);
                            }
                        }
                    }
                }
                Ok(())
            })
            .context("Tola could not start its after-publish worker; restart `tola dev`")?;
        Ok(Self {
            sender: HookSender {
                mailbox,
                cancellation: cancellation.token(),
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
                .map_err(|_| anyhow!("Tola could not stop its after-publish worker"))??;
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

/// The latest completed after-publish run: what it consumed, and the failure it reported.
///
/// A later revision with equal dependencies reports this outcome again instead of running the
/// commands: materializing the site for them writes enough files that macOS may coalesce the
/// writes into a rescan of the whole site, and running again for that rescan's unchanged
/// rebuild would never settle.
struct CompletedRun {
    dependencies: tola_build::hooks::AfterPublishDependencies,
    failure: Option<Vec<Diagnostic>>,
}

/// Run one revision's after-publish commands under its published identity.
///
/// The span is entered for the synchronous command run, so a command's hook events name their
/// round and revision even while a later round publishes.
fn consume_revision(
    pending: &PendingRevision,
    cancellation: &BuildCancellation,
    events: super::events::Events,
) -> Result<Result<Vec<Diagnostic>>> {
    let site = pending.site.as_ref();
    let span = tracing::info_span!(
        target: "tola::dev",
        "after-publish",
        round = pending.round,
        revision = site.manifest().revision().as_str(),
    );
    let mut event_write_failed = false;
    let outcome = span.in_scope(|| {
        let files = match site.hook_output(cancellation) {
            Ok(files) => files,
            Err(error) => {
                if !events.0 {
                    return Err(error);
                }
                let (status, diagnostics) = hook_failure(&error);
                if let Err(write_error) = events.hook_finished(
                    None,
                    site.manifest().revision().as_str(),
                    status,
                    &diagnostics,
                ) {
                    event_write_failed = true;
                    return Err(write_error);
                }
                return Err(error);
            }
        };
        tola_build::hooks::run_after_publish_hooks_observed(
            site.config(),
            site.mode(),
            files.root(),
            cancellation,
            |hook, outcome| {
                if !events.0 {
                    return Ok(());
                }
                let (status, diagnostics) = match outcome {
                    Ok(diagnostics) => ("succeeded", diagnostics.clone()),
                    Err(error) => hook_failure(error),
                };
                let written = events.hook_finished(
                    Some(hook),
                    site.manifest().revision().as_str(),
                    status,
                    &diagnostics,
                );
                event_write_failed = written.is_err();
                written
            },
        )
    });
    if event_write_failed {
        return Err(outcome.expect_err("event write failed"));
    }
    Ok(outcome)
}

fn hook_failure(error: &anyhow::Error) -> (&'static str, Vec<Diagnostic>) {
    if tola_build::cancellation::is_cancelled(error) {
        ("cancelled", Vec::new())
    } else {
        (
            "failed",
            crate::cli::output::attached_or_fallback(error, crate::codes::hook::AFTER_PUBLISH),
        )
    }
}

fn after_publish_failure(error: &anyhow::Error) -> Diagnostic {
    tola_diagnostic::attached(error)
        .and_then(|diagnostics| {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.severity == tola_diagnostic::Severity::Error)
                .cloned()
        })
        .unwrap_or_else(|| tola_diagnostic::fallback(crate::codes::hook::AFTER_PUBLISH, error))
        .with_help(
            "Fix the failing command in `build.hooks.after-publish`, then change the site or one of its `rerun-on` paths to run it again",
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
        let cancellation = crate::cancellation::Cancellation::default();
        let queue = HookQueue::start(
            output,
            None,
            cancellation.clone(),
            super::super::events::Events(false),
        )
        .unwrap();
        let sender = queue.sender();
        let (completed, completion) = mpsc::channel();
        let closing = std::thread::spawn(move || completed.send(queue.finish()).unwrap());

        let closed = completion.recv_timeout(std::time::Duration::from_secs(2));
        drop(sender);
        closing.join().unwrap();

        closed
            .expect("closing waited for a retained sender")
            .unwrap();
        assert!(!cancellation.is_requested());
    }
}
