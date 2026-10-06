//! Which job each lane runs next, when a due check is dispatched, and when an idle connection lets
//! its compiled site go.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use tola_build::diagnostic::Diagnostic;

use crate::compiler::{AnalysisRequest, CheckRequest, SourceJob};
use crate::protocol::CheckProgress;

use super::{Connection, Phase};

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

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// The next job the compiler worker may run, at `now`.
    pub(crate) fn next_job(&mut self, now: Instant) -> Option<SourceJob> {
        // A check past its bound is dispatched before the queued jobs, which keep their order and
        // are served once it finishes; a check a newer revision superseded is replaced, never run.
        let starved = self
            .check_due
            .is_some_and(|due| now >= due + CHECK_STARVATION_LIMIT);
        if !starved {
            while let Some(job) = self.queries.pop_front() {
                if !job.is_cancelled() {
                    self.released = false;
                    return Some(job);
                }
            }
        }
        if std::mem::take(&mut self.release) {
            self.released = true;
            return Some(SourceJob::ReleaseIdle);
        }
        if self.check_due.is_some_and(|due| now < due) {
            return None;
        }
        self.check_due = None;
        let pending = self
            .check
            .take()
            .filter(|check| !check.sources.cancellation.is_cancelled())?;
        self.running_check = Some(pending.checked_revision);
        self.released = false;
        // A report is timed from the moment its check started running, so a check that answers
        // inside the window never shows one, and the first check shows one at once.
        self.begin_progress(now);
        Some(SourceJob::Check(CheckRequest {
            checked_revision: pending.checked_revision,
            sources: pending.sources,
            view: self.open.view(),
        }))
    }

    pub(crate) fn next_analysis(&mut self) -> Option<AnalysisRequest> {
        while let Some(request) = self.analyses.pop_front() {
            if !request.cancellation.is_cancelled() {
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
        if self.release || self.released {
            return;
        }
        let busy = self.check.is_some()
            || self.running_check.is_some()
            || !self.requests.is_empty()
            || !self.queries.is_empty()
            || !self.analyses.is_empty();
        if busy {
            self.active = now;
            return;
        }
        let idle = now.duration_since(self.active) >= IDLE_RELEASE_DELAY;
        if self.overrides.is_empty() || idle {
            self.release = true;
        }
    }

    /// The check progress a query is answered in, which only this connection can tell apart.
    pub(super) fn check_progress(&self) -> CheckProgress {
        if self.check_due.is_some()
            || self.check.is_some()
            || self.running_check == Some(self.revision)
        {
            CheckProgress::Checking
        } else if let Some((revision, compiled)) = self.checked
            && revision == self.revision
        {
            if compiled {
                CheckProgress::Checked
            } else {
                CheckProgress::Failed
            }
        } else {
            CheckProgress::NotChecked
        }
    }

    pub(super) fn report_source_check(&mut self, state: CheckProgress) -> Result<()> {
        if !matches!(&self.phase, Phase::Ready(workspace) if workspace.source_check_status) {
            return Ok(());
        }
        self.notify(
            "tola/sourceCheckStatus",
            serde_json::json!({ "revision": self.revision, "state": state }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::{CheckedSources, SourceCompilation, SourceFailure};
    use crate::connection::tests::{
        hover_query, messages, open_document, ready, ready_at, ready_workspace,
    };
    use crate::protocol::CheckMode;
    use lsp_types::notification::{self, Notification as LspNotification};
    use serde_json::json;
    use std::sync::Arc;
    use tola_typst_syntax::names::SelectedInterfaces;

    use lsp_server::{Message, Notification, RequestId};

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

    /// A check that has waited out its bound is dispatched ahead of the queued jobs, which keep
    /// their order and are served once it finishes.
    #[test]
    fn starved_check_runs_before_queued_jobs() {
        let mut connection = ready(Vec::new());
        connection.sources_changed(false).unwrap();
        let due = connection.check_due.expect("a queued check");
        connection.query(RequestId::from(1), hover_query()).unwrap();
        connection.query(RequestId::from(2), hover_query()).unwrap();
        assert!(
            matches!(connection.next_job(due), Some(SourceJob::Query(_))),
            "a fresh queue is served first"
        );
        assert!(
            matches!(
                connection.next_job(due + CHECK_STARVATION_LIMIT),
                Some(SourceJob::Check(_))
            ),
            "a check past its bound outruns the queue"
        );
        assert_eq!(connection.queries.len(), 1, "the queued job waits its turn");
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

    #[test]
    fn source_status_ignores_superseded_completion() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.source_check_status = true;
        connection.sources_changed(false).unwrap();
        let older = connection.revision;
        connection.next_job(Instant::now()).unwrap();
        connection.sources_changed(false).unwrap();
        let current = connection.revision;
        messages(&mut connection);
        connection
            .completed(SourceCompilation::Checked {
                checked_revision: older,
                checked: Ok(CheckedSources {
                    configuration: None,
                    compiled: true,
                    diagnostics: Vec::new(),
                    read_paths: None,
                    selected: Arc::new(SelectedInterfaces::default()),
                }),
            })
            .unwrap();
        assert!(messages(&mut connection).is_empty());
        assert_eq!(connection.check_progress(), CheckProgress::Checking);
        connection.next_job(Instant::now()).unwrap();
        connection
            .completed(SourceCompilation::Checked {
                checked_revision: current,
                checked: Ok(CheckedSources {
                    configuration: None,
                    compiled: false,
                    diagnostics: Vec::new(),
                    read_paths: None,
                    selected: Arc::new(SelectedInterfaces::default()),
                }),
            })
            .unwrap();
        let states = messages(&mut connection)
            .into_iter()
            .filter_map(|message| match message {
                Message::Notification(notification)
                    if notification.method == "tola/sourceCheckStatus" =>
                {
                    Some(notification.params)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(states, [json!({"revision":current,"state":"failed"})]);
    }

    #[test]
    fn cancelled_check_is_not_failure() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.source_check_status = true;
        workspace.check_mode = CheckMode::OnSave;
        connection.sources_changed(false).unwrap();
        let older = connection.revision;
        connection.next_job(Instant::now()).unwrap();
        connection.sources_changed(true).unwrap();
        let current = connection.revision;
        let states = messages(&mut connection)
            .into_iter()
            .filter_map(|message| match message {
                Message::Notification(notification)
                    if notification.method == "tola/sourceCheckStatus" =>
                {
                    Some(notification.params)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            states.last(),
            Some(&json!({"revision":current,"state":"notChecked"}))
        );
        connection
            .completed(SourceCompilation::Checked {
                checked_revision: older,
                checked: Err(SourceFailure::Cancelled),
            })
            .unwrap();
        assert!(messages(&mut connection).is_empty());
        assert_eq!(connection.check_progress(), CheckProgress::NotChecked);
    }
}
