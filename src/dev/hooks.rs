//! After-publish commands with one latest pending development revision.

use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, anyhow};
use tola_build::cancellation::{BuildCancellation, BuildCanceller, is_cancelled};
use tola_build::hooks::HookExecutor;
use tola_build::site::SiteRevision;
use tola_diagnostic::Diagnostic;

#[derive(Clone)]
pub(crate) struct HookSender {
    mailbox: Arc<HookMailbox>,
    cancellation: BuildCancellation,
}

struct PendingRevision {
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
    fn next(&self, cancellation: &BuildCancellation) -> Option<PendingRevision> {
        let mut pending = self
            .mailbox
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if cancellation.is_cancelled() {
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
    pub(crate) fn enqueue(&self, site: Arc<SiteRevision>, round: Option<u64>) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Ok(());
        }
        let participates = tola_build::hooks::has_after_publish_hooks(site.config(), site.mode());
        let discarded = {
            let mut pending = self
                .mailbox
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
            if matches!(*pending, PendingHooks::Closed) {
                if !participates {
                    return Ok(());
                }
                return Err(anyhow!(
                    "Tola could no longer run after-publish commands; restart `tola dev`"
                ));
            }
            std::mem::replace(
                &mut *pending,
                if participates {
                    PendingHooks::Revision(PendingRevision { round, site })
                } else {
                    PendingHooks::Waiting
                },
            )
        };
        self.mailbox.available.notify_one();
        drop(discarded);
        Ok(())
    }
}

/// Latest-wins queue: the running hook finishes while newer builds replace the pending slot.
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
        executor: HookExecutor,
    ) -> Result<Self> {
        let mailbox = Arc::new(HookMailbox {
            pending: Mutex::new(PendingHooks::Waiting),
            available: Condvar::new(),
        });
        let pending = HookReceiver {
            mailbox: Arc::clone(&mailbox),
        };
        let shutdown = BuildCanceller::child(&cancellation.token());
        let worker_token = shutdown.token();
        let worker_cancellation = cancellation.clone();
        let worker = std::thread::Builder::new()
            .name("tola-after-publish".into())
            .spawn(move || {
                let mut completed: Option<CompletedRun> = None;
                while let Some(pending) = pending.next(&worker_token) {
                    let rev = pending.site.manifest().revision().as_str();
                    let deps = pending.site.after_publish_dependencies(&worker_token);
                    if worker_token.is_cancelled() {
                        break;
                    }
                    match deps {
                        Ok(deps)
                            if completed
                                .as_ref()
                                .is_some_and(|run| run.dependencies == deps) =>
                        {
                            tracing::debug!(
                                target: "tola::dev",
                                revision = rev,
                                "kept the previous after-publish outcome for unchanged dependencies"
                            );
                            if let Some(diags) =
                                completed.as_ref().and_then(|run| run.failure.as_ref())
                            {
                                report_after_publish_failure(
                                    &output,
                                    browser.as_ref(),
                                    &pending.site,
                                    diags,
                                    diags.first(),
                                );
                            }
                        }
                        Ok(deps) => {
                            let outcome = match consume_revision(
                                &pending,
                                &worker_token,
                                &executor,
                                events,
                            ) {
                                Ok(outcome) => outcome,
                                Err(err) => {
                                    worker_cancellation.request();
                                    return Err(err);
                                }
                            };
                            if worker_token.is_cancelled() {
                                break;
                            }
                            let failure = match outcome {
                                Ok(diags) => {
                                    let _ = output.diagnostics(&diags);
                                    None
                                }
                                Err(err) => {
                                    if is_cancelled(&err) {
                                        break;
                                    }
                                    tracing::debug!(
                                        target: "tola::dev",
                                        revision = rev,
                                        %err,
                                        "after-publish command failed"
                                    );
                                    let diag = after_publish_failure(&err);
                                    let diags = crate::cli::output::attached_or_fallback(
                                        &err,
                                        crate::codes::hook::AFTER_PUBLISH,
                                    );
                                    report_after_publish_failure(
                                        &output,
                                        browser.as_ref(),
                                        &pending.site,
                                        &diags,
                                        Some(&diag),
                                    );
                                    Some(diags)
                                }
                            };
                            completed = Some(CompletedRun {
                                dependencies: deps,
                                failure,
                            });
                        }
                        Err(err) => {
                            completed = None;
                            let diag = after_publish_failure(&err);
                            let diags = crate::cli::output::attached_or_fallback(
                                &err,
                                crate::codes::hook::AFTER_PUBLISH,
                            );
                            report_after_publish_failure(
                                &output,
                                browser.as_ref(),
                                &pending.site,
                                &diags,
                                Some(&diag),
                            );
                        }
                    }
                }
                Ok(())
            })
            .context("Tola could not start its after-publish worker; restart `tola dev`")?;
        Ok(Self {
            sender: HookSender {
                mailbox,
                cancellation: shutdown.token(),
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
        if let Err(err) = self.close() {
            tracing::error!(target: "tola::dev", %err, "after-publish shutdown failed");
        }
    }
}

fn report_after_publish_failure(
    output: &crate::cli::output::CommandOutput,
    browser: Option<&crate::dev::reload::transport::ReloadDiagnostics>,
    site: &Arc<SiteRevision>,
    diags: &[Diagnostic],
    browser_diag: Option<&Diagnostic>,
) {
    let _ = output.status("Site built; an after-publish command failed");
    let _ = output.diagnostics(diags);
    if let (Some(browser), Some(diag)) = (browser, browser_diag) {
        browser.after_publish_failed(site, diag);
    }
}

/// Reused across revisions with equal dependencies so hook writes do not cause a rescan loop.
struct CompletedRun {
    dependencies: tola_build::hooks::AfterPublishDependencies,
    failure: Option<Vec<Diagnostic>>,
}

fn consume_revision(
    pending: &PendingRevision,
    cancellation: &BuildCancellation,
    executor: &HookExecutor,
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
            Err(err) => {
                if !events.0 {
                    return Err(err);
                }
                let (status, diags) = hook_failure(&err);
                if let Err(write_err) =
                    events.hook_finished(None, site.manifest().revision().as_str(), status, &diags)
                {
                    event_write_failed = true;
                    return Err(write_err);
                }
                return Err(err);
            }
        };
        tola_build::hooks::run_after_publish_hooks_observed(
            site.config(),
            site.mode(),
            site.scope(),
            files.root(),
            cancellation,
            executor,
            |hook, outcome| {
                if !events.0 {
                    return Ok(());
                }
                let (status, diags) = match outcome {
                    Ok(diags) => ("succeeded", diags.clone()),
                    Err(err) => hook_failure(err),
                };
                let written = events.hook_finished(
                    Some(hook),
                    site.manifest().revision().as_str(),
                    status,
                    &diags,
                );
                event_write_failed |= written.is_err();
                written
            },
        )
    });
    if event_write_failed {
        return Err(outcome.expect_err("event write failed"));
    }
    Ok(outcome)
}

fn hook_failure(err: &anyhow::Error) -> (&'static str, Vec<Diagnostic>) {
    if is_cancelled(err) {
        (
            "cancelled",
            tola_diagnostic::attached(err).unwrap_or_default().to_vec(),
        )
    } else {
        (
            "failed",
            crate::cli::output::attached_or_fallback(err, crate::codes::hook::AFTER_PUBLISH),
        )
    }
}

fn after_publish_failure(err: &anyhow::Error) -> Diagnostic {
    tola_diagnostic::attached(err)
        .and_then(|diags| {
            diags
                .iter()
                .find(|diag| diag.severity == tola_diagnostic::Severity::Error)
                .cloned()
        })
        .unwrap_or_else(|| tola_diagnostic::fallback(crate::codes::hook::AFTER_PUBLISH, err))
        .with_help(
            "Fix the failing command in `hooks.after-publish`, then change the site or one of its `rerun-on` paths to run it again",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn queue_shutdown_local() {
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
            HookExecutor::new(),
        )
        .unwrap();
        let sender = queue.sender();
        let (tx, rx) = mpsc::channel();
        let closing = std::thread::spawn(move || tx.send(queue.finish()).unwrap());

        let closed = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(sender);
        closing.join().unwrap();

        closed.expect("closing waited for retained sender").unwrap();
        assert!(!cancellation.is_requested());
    }
}
