//! Which job each lane runs next, when a due check is dispatched, and when an idle connection lets
//! its compiled site go.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use tola_diagnostic::Diagnostic;

use super::{Connection, IdleRelease};
use crate::compiler::{AnalysisRequest, CheckRequest, SourceJob};

/// How long typing must pause before a whole-site check is worth its cost.
///
/// A check spends the whole site's work whatever changed, so one check per pause keeps a large
/// site's diagnostics fresh while one per keystroke only repeats work on revisions the author has
/// already left behind.
pub(super) const CHECK_SETTLE_DELAY: Duration = Duration::from_millis(400);

/// How long a due check may wait behind queued client jobs before it is dispatched ahead of them.
///
/// A check spends the whole site's work, and a client that keeps the compiler lane busy — semantic
/// tokens and hovers after every edit — would otherwise postpone the author's diagnostics for as
/// long as it keeps asking; past this bound the check's freshness outranks the queue's order.
const CHECK_STARVATION_LIMIT: Duration = Duration::from_secs(2);

/// How long a connection that is answering may hold its compiled site before it lets it go.
///
/// The held compilation is a whole site's Bundle, which is the largest thing one connection
/// retains. Answering again re-derives it.
const IDLE_RELEASE_DELAY: Duration = Duration::from_secs(300);

/// One lane's waiting jobs, each with the moment it was queued.
///
/// The queue owns the timestamp, so the enqueue that pushes a job records it in the same place and
/// dispatch reads the wait without a parallel table of timestamps.
pub(super) struct JobQueue<J> {
    queued: VecDeque<QueuedJob<J>>,
}

/// One job waiting for its lane, with the moment it was queued.
struct QueuedJob<J> {
    queued_at: Instant,
    job: J,
}

impl<J> JobQueue<J> {
    pub(super) fn new() -> Self {
        Self {
            queued: VecDeque::new(),
        }
    }

    pub(super) fn push_back(&mut self, job: J) {
        self.queued.push_back(QueuedJob {
            queued_at: Instant::now(),
            job,
        });
    }

    /// Take the oldest job, with the time it waited for its lane.
    pub(super) fn pop_front(&mut self) -> Option<(J, Duration)> {
        let QueuedJob { queued_at, job } = self.queued.pop_front()?;
        Some((job, queued_at.elapsed()))
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&J) -> bool) {
        self.queued.retain(|queued| keep(&queued.job));
    }

    pub(super) fn clear(&mut self) {
        self.queued.clear();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// The next job the compiler worker may run, at `now`.
    pub(crate) fn next_job(&mut self, now: Instant) -> Option<SourceJob> {
        // A check past its bound is dispatched before the queued jobs, which keep their order and
        // are served once it finishes; a check a newer revision superseded is replaced, never run.
        let starved = self
            .check
            .as_ref()
            .is_some_and(|check| now >= check.due + CHECK_STARVATION_LIMIT);
        if !starved {
            while let Some((job, waited)) = self.queries.pop_front() {
                if !job.is_cancelled() {
                    self.resume_after_release();
                    log_dispatched(&job, waited);
                    return Some(job);
                }
            }
        }
        if self.idle_release == IdleRelease::Requested {
            self.idle_release = IdleRelease::Released;
            return Some(SourceJob::ReleaseIdle);
        }
        if self.check.as_ref().is_some_and(|check| now < check.due) {
            return None;
        }
        let pending = self
            .check
            .take()
            .filter(|check| !check.sources.cancellation.is_cancelled())?;
        let checked_revision = pending.checked_revision;
        let waited = now.saturating_duration_since(pending.queued_at);
        self.running_check = Some(checked_revision);
        self.resume_after_release();
        // A report is timed from the moment its check started running, so a check that answers
        // inside the window never shows one, and the first check shows one at once.
        self.begin_progress(now);
        let job = SourceJob::Check(CheckRequest {
            checked_revision,
            sources: pending.sources,
            view: self.open.view(),
        });
        // A check is a pending revision rather than a queue entry: its wait is measured from the
        // moment the revision became pending, settle delay included.
        tracing::debug!(
            lane = "compiler",
            kind = job.kind(),
            checked_revision,
            wait_ms = millis(waited),
            "lane job dispatched"
        );
        Some(job)
    }

    /// Record that a job now holds the lane: a released site is about to be derived again, so
    /// letting it go is worth asking another time.
    fn resume_after_release(&mut self) {
        if self.idle_release == IdleRelease::Released {
            self.idle_release = IdleRelease::Available;
        }
    }

    pub(crate) fn next_analysis(&mut self) -> Option<AnalysisRequest> {
        while let Some((request, waited)) = self.analyses.pop_front() {
            if !request.sources.cancellation.is_cancelled() {
                log_analysis_dispatched(&request, waited);
                return Some(request);
            }
        }
        None
    }

    /// The work this connection owes the client between jobs: opening a progress report that is
    /// due, and letting go of a compiled site nothing is using.
    pub(crate) fn run_due_work(&mut self, now: Instant) -> Result<()> {
        self.open_progress(now)?;
        self.release_idle(now);
        Ok(())
    }

    /// Let go of the compiled site when nothing is answering from it.
    ///
    /// The compiled site is the largest thing a connection retains and the cheapest to derive
    /// again, so a connection with no open document lets it go at once, and one that has answered
    /// nothing for [`IDLE_RELEASE_DELAY`] lets it go as well. The next request derives it again.
    fn release_idle(&mut self, now: Instant) {
        if self.idle_release != IdleRelease::Available {
            return;
        }
        let busy = self.check.is_some()
            || self.running_check.is_some()
            || !self.requests.is_empty()
            || !self.queries.is_empty()
            || !self.analyses.is_empty();
        if busy {
            self.last_work = now;
            return;
        }
        let idle = now.duration_since(self.last_work) >= IDLE_RELEASE_DELAY;
        if self.open.overrides().is_empty() || idle {
            self.idle_release = IdleRelease::Requested;
            tracing::debug!(
                lane = "compiler",
                idle_ms = millis(now.duration_since(self.last_work)),
                open_sources = self.open.overrides().len(),
                "an idle connection asks its compiler lane to release the compiled site"
            );
        }
    }
}

/// Log one compiler-lane job as it leaves its queue: what it is and how long it waited.
fn log_dispatched(job: &SourceJob, waited: Duration) {
    tracing::debug!(
        lane = "compiler",
        kind = job.kind(),
        checked_revision = job.checked_revision(),
        serial = job.serial(),
        wait_ms = millis(waited),
        "lane job dispatched"
    );
}

/// Log one analysis-lane request as it leaves its queue.
fn log_analysis_dispatched(request: &AnalysisRequest, waited: Duration) {
    tracing::debug!(
        lane = "analysis",
        kind = request.kind(),
        serial = request.serial,
        wait_ms = millis(waited),
        "lane job dispatched"
    );
}

/// One wait, in whole milliseconds, as the logs record it.
fn millis(waited: Duration) -> u64 {
    u64::try_from(waited.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{open_document, ready, ready_at, ready_workspace};
    use lsp_types::notification::{self, Notification as LspNotification};
    use serde_json::json;

    use crate::protocol::CheckMode;
    use lsp_server::Notification;

    /// Only typing defers its check: a save checks at once, so the site answers a deliberate edit
    /// without waiting out a pause the author never made.
    #[test]
    fn only_typing_defers_its_check() {
        let root = std::env::current_dir().unwrap();
        let uri = crate::uri::from_file_path(&root.join("content/document.typ")).unwrap();
        let text_document = |version: i32| json!({ "uri": uri.as_str(), "version": version });
        let mut connection = ready(Vec::new());
        open_document(&mut connection, &uri, 1, "Body\n");

        let before = Instant::now();
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.into(),
                json!({
                    "textDocument": text_document(2),
                    "contentChanges": [{ "text": "Body\nmore\n" }],
                }),
            ))
            .unwrap();
        assert!(
            connection.next_job(before).is_none(),
            "a keystroke waits for the pause"
        );
        assert!(
            connection
                .next_job(before + CHECK_SETTLE_DELAY * 2)
                .is_some(),
            "the check runs once typing pauses"
        );

        let _ = connection
            .notification(Notification::new(
                notification::DidSaveTextDocument::METHOD.into(),
                json!({ "textDocument": text_document(2) }),
            ))
            .unwrap();
        assert!(
            connection.next_job(Instant::now()).is_some(),
            "a save checks without waiting"
        );
    }
    /// A site checked on save advances its revision with every edit but runs no check for one:
    /// typing never spends a whole site's work, and the save is what checks.
    #[test]
    fn on_save_mode_checks_only_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("page.typ");
        std::fs::write(&path, "= Page\n").unwrap();
        let uri = crate::uri::from_file_path(&path).unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let workspace = ready_workspace(&mut connection);
        workspace.check_mode = CheckMode::OnSave;
        open_document(&mut connection, &uri, 1, "= Page\n");
        // Opening a document is a change like any other, and this mode checks it.
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str(),"version":2},"contentChanges":[{"text":"= Page\n\n#let draft = 1\n"}]}),
            ))
            .unwrap();
        // A typing change settles before a check of the type mode would run, so the moment is read
        // past that delay: this mode runs no check at all.
        assert!(
            connection
                .next_job(Instant::now() + CHECK_SETTLE_DELAY * 2)
                .is_none()
        );
        let _ = connection
            .notification(Notification::new(
                notification::DidSaveTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()}}),
            ))
            .unwrap();
        assert!(matches!(
            connection.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
    }
    /// A released site waits for a job that derives it again: asking on every idle turn would
    /// release nothing.
    #[test]
    fn released_site_waits_for_new_work() {
        let mut connection = ready(Vec::new());
        let now = Instant::now();
        connection.run_due_work(now).unwrap();
        assert!(matches!(
            connection.next_job(now),
            Some(SourceJob::ReleaseIdle)
        ));
        assert!(connection.next_job(now).is_none());

        let uri = crate::uri::from_file_path(
            &std::env::current_dir()
                .unwrap()
                .join("content/document.typ"),
        )
        .unwrap();
        open_document(&mut connection, &uri, 1, "Body\n");
        let derived_at = Instant::now();
        assert!(matches!(
            connection.next_job(derived_at),
            Some(SourceJob::Check(_))
        ));
        connection.running_check = None;
        let idle_at = derived_at + IDLE_RELEASE_DELAY;
        connection.run_due_work(idle_at).unwrap();
        assert!(matches!(
            connection.next_job(idle_at),
            Some(SourceJob::ReleaseIdle)
        ));
    }
}
