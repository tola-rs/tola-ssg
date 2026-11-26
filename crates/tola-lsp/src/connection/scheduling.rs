//! Lane job scheduling, check starvation bounds, and idle cache release.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use tola_diagnostic::Diagnostic;

use super::Connection;
use crate::compiler::{AnalysisRequest, CheckRequest, SourceJob};

pub(super) const CHECK_SETTLE_DELAY: Duration = Duration::from_millis(400);

/// Bound how long a due check can starve behind continuous editor queries.
const CHECK_STARVATION_LIMIT: Duration = Duration::from_secs(2);

const IDLE_RELEASE_DELAY: Duration = Duration::from_secs(300);

pub(super) struct JobQueue<J> {
    queued: VecDeque<QueuedJob<J>>,
}

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
    pub(crate) fn next_job(&mut self, now: Instant) -> Option<SourceJob> {
        let starved = self
            .check
            .as_ref()
            .is_some_and(|check| now >= check.due + CHECK_STARVATION_LIMIT);
        if !starved {
            while let Some((job, waited)) = self.queries.pop_front() {
                if !job.is_cancelled() {
                    self.last_work = Some(now);
                    log_dispatched(
                        "compiler",
                        job.kind(),
                        job.checked_revision(),
                        job.serial(),
                        waited,
                    );
                    return Some(job);
                }
            }
        }
        if self.check.as_ref().is_some_and(|check| now < check.due) {
            return None;
        }
        let pending = self
            .check
            .take()
            .filter(|check| !check.sources.cancellation.is_cancelled())?;
        let checked_rev = pending.checked_revision;
        let waited = now.saturating_duration_since(pending.queued_at);
        self.running_check = Some(checked_rev);
        self.last_work = Some(now);
        self.begin_progress(now);
        let job = SourceJob::Check(CheckRequest {
            checked_revision: checked_rev,
            sources: pending.sources,
            view: self.open.view(),
        });
        log_dispatched("compiler", job.kind(), Some(checked_rev), None, waited);
        Some(job)
    }

    pub(crate) fn next_analysis(&mut self) -> Option<AnalysisRequest> {
        while let Some((req, waited)) = self.analyses.pop_front() {
            if !req.sources.cancellation.is_cancelled() {
                self.last_work = Some(Instant::now());
                log_dispatched("analysis", req.kind(), None, Some(req.serial), waited);
                return Some(req);
            }
        }
        None
    }

    pub(crate) fn run_due_work(&mut self, now: Instant, lanes_busy: bool) -> Result<bool> {
        self.open_progress(now)?;
        Ok(self.release_idle(now, lanes_busy))
    }

    fn release_idle(&mut self, now: Instant, lanes_busy: bool) -> bool {
        let Some(last_work) = self.last_work else {
            return false;
        };
        let busy = lanes_busy
            || self.check.is_some()
            || self.running_check.is_some()
            || !self.requests.is_empty()
            || !self.queries.is_empty()
            || !self.analyses.is_empty();
        if busy {
            self.last_work = Some(now);
            return false;
        }
        let idle_for = now.duration_since(last_work);
        let open_sources = self.open.overrides().len();
        if open_sources != 0 && idle_for < IDLE_RELEASE_DELAY {
            return false;
        }
        self.last_work = None;
        tracing::debug!(
            idle_ms = millis(idle_for),
            open_sources,
            "an idle connection releases its worker caches"
        );
        true
    }
}

fn log_dispatched(
    lane: &'static str,
    kind: &'static str,
    checked_revision: Option<u64>,
    serial: Option<u64>,
    waited: Duration,
) {
    tracing::debug!(
        lane,
        kind,
        checked_revision,
        serial,
        wait_ms = millis(waited),
        "lane job dispatched"
    );
}

fn millis(waited: Duration) -> u64 {
    u64::try_from(waited.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{open_document, ready, ready_at, ready_workspace};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;

    use crate::protocol::CheckMode;
    use lsp_server::{Notification, Request};

    #[test]
    fn only_typing_defers_its_check() {
        let root = std::env::current_dir().unwrap();
        let uri = crate::uri::from_file_path(&root.join("content/document.typ")).unwrap();
        let text_doc = |rev: i32| json!({ "uri": uri.as_str(), "version": rev });
        let mut conn = ready(Vec::new());
        open_document(&mut conn, &uri, 1, "Body\n");

        let before = Instant::now();
        let _ = conn
            .notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.into(),
                json!({
                    "textDocument": text_doc(2),
                    "contentChanges": [{ "text": "Body\nmore\n" }],
                }),
            ))
            .unwrap();
        assert!(conn.next_job(before).is_none());
        assert!(conn.next_job(before + CHECK_SETTLE_DELAY * 2).is_some());

        let _ = conn
            .notification(Notification::new(
                notification::DidSaveTextDocument::METHOD.into(),
                json!({ "textDocument": text_doc(2) }),
            ))
            .unwrap();
        assert!(conn.next_job(Instant::now()).is_some());
    }

    #[test]
    fn on_save_mode_checks_only_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.typ");
        std::fs::write(&path, "= Page\n").unwrap();
        let uri = crate::uri::from_file_path(&path).unwrap();
        let mut conn = ready_at(Vec::new(), dir.path().to_path_buf());
        ready_workspace(&mut conn).check_mode = CheckMode::OnSave;
        open_document(&mut conn, &uri, 1, "= Page\n");
        assert!(matches!(
            conn.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
        let _ = conn
            .notification(Notification::new(
                notification::DidChangeTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str(),"version":2},"contentChanges":[{"text":"= Page\n\n#let draft = 1\n"}]}),
            ))
            .unwrap();
        assert!(
            conn.next_job(Instant::now() + CHECK_SETTLE_DELAY * 2)
                .is_none()
        );
        let _ = conn
            .notification(Notification::new(
                notification::DidSaveTextDocument::METHOD.into(),
                json!({"textDocument":{"uri":uri.as_str()}}),
            ))
            .unwrap();
        assert!(matches!(
            conn.next_job(Instant::now()),
            Some(SourceJob::Check(_))
        ));
    }

    #[test]
    fn idle_release_resets_after_work() {
        let mut conn = ready(Vec::new());
        assert!(!conn.run_due_work(Instant::now(), false).unwrap());

        let uri = crate::uri::from_file_path(
            &std::env::current_dir()
                .unwrap()
                .join("content/document.typ"),
        )
        .unwrap();
        open_document(&mut conn, &uri, 1, "Body\n");
        let derived_at = Instant::now();
        assert!(matches!(
            conn.next_job(derived_at),
            Some(SourceJob::Check(_))
        ));
        conn.running_check = None;
        let idle_at = derived_at + IDLE_RELEASE_DELAY;
        assert!(conn.run_due_work(idle_at, false).unwrap());
        assert!(!conn.run_due_work(idle_at, false).unwrap());

        conn.request(Request::new(
            1.into(),
            request::SemanticTokensFullRequest::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str()}}),
        ))
        .unwrap();
        crate::connection::tests::finish_analysis(&mut conn);
        let idle_at = Instant::now() + IDLE_RELEASE_DELAY;
        assert!(conn.run_due_work(idle_at, false).unwrap());
    }
}
