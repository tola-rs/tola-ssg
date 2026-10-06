//! The work-done report a running check opens: when it is due, the client's creation answer that
//! opens it, and the reports that give it up.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress,
    WorkDoneProgressCreateParams, WorkDoneProgressEnd,
};
use tola_build::diagnostic::Diagnostic;

use super::{Connection, PendingCreation, Phase, Progress};

/// How long a check may run before the client is told that it is running.
///
/// A check that finishes inside this window reports nothing: the report would arrive after the
/// author stopped waiting for it, and a client showing one has nothing to show.
const PROGRESS_DELAY: Duration = Duration::from_millis(300);

/// How long a report's creation request may stay unanswered before the report is given up.
///
/// The answer normally arrives in the client's next turn, so a report the author has waited this
/// long for is worth no more of the connection's state — and one client's unanswered creation must
/// not hold back the reports of every later check.
const PROGRESS_CREATION_TIMEOUT: Duration = Duration::from_secs(10);

/// What the report a running check is shown under says, beside the client's own progress spinner.
pub(super) const PROGRESS_TITLE: &str = "Tola is checking the site";

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// Open the report one check runs under.
    ///
    /// The report exists only once the check has run for [`PROGRESS_DELAY`], so a site that checks
    /// in a moment shows nothing; a check that finishes first ends a report that was never opened,
    /// and the client hears nothing at all.
    pub(super) fn begin_progress(&mut self, now: Instant) {
        let Phase::Ready(workspace) = &self.phase else {
            return;
        };
        if !workspace.features.work_done_progress || !self.progress_agreed {
            return;
        }
        let revision = self.revision;
        let due = if self.warm { now + PROGRESS_DELAY } else { now };
        self.request_serial += 1;
        self.progress = Some(Progress {
            revision,
            token: NumberOrString::String(format!("tola-check-{}", self.request_serial)),
            due,
            opened: false,
        });
    }

    /// Tell the client the check is running, once it has run long enough to be worth showing.
    pub(super) fn open_progress(&mut self, now: Instant) -> Result<()> {
        let Some(progress) = self.progress.as_ref() else {
            return Ok(());
        };
        if progress.opened || now < progress.due {
            return Ok(());
        }
        let token = progress.token.clone();
        // This report's own creation is still within its bound: the client's answer is what opens
        // the report, so nothing more is asked for it.
        if self.progress_create.as_ref().is_some_and(|creation| {
            creation.token == token && now < creation.sent + PROGRESS_CREATION_TIMEOUT
        }) {
            return Ok(());
        }
        if let Some(creation) = self.progress_create.take()
            && creation.token == token
        {
            // The client has not answered this report's own creation within its bound, so the
            // report is given up: the answer is matched by request id, so it reaches nothing.
            let waited = now.saturating_duration_since(creation.sent);
            tracing::warn!(
                token = ?creation.token,
                waited = ?waited,
                "a progress creation the client did not answer was dropped"
            );
            self.progress = None;
            return Ok(());
        }
        let id = self.send_request(
            request::WorkDoneProgressCreate::METHOD,
            WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        )?;
        self.progress_create = Some(PendingCreation {
            id,
            token,
            sent: now,
        });
        Ok(())
    }

    /// End the report one revision's check opened, when the report is that check's.
    ///
    /// A superseded check finishes after the newer one started, so its report is already closed and
    /// the newer report is not its to end.
    pub(super) fn finish_progress(&mut self, revision: u64) -> Result<()> {
        if self
            .progress
            .as_ref()
            .is_some_and(|progress| progress.revision == revision)
        {
            self.end_progress()?;
        }
        Ok(())
    }

    /// End the report the client is being shown, if it is shown one.
    pub(super) fn end_progress(&mut self) -> Result<()> {
        let Some(progress) = self.progress.take() else {
            return Ok(());
        };
        // A creation the client has not answered belongs to the report that asked for it: it goes
        // with that report, so its answer cannot reach a later one.
        if self
            .progress_create
            .as_ref()
            .is_some_and(|creation| creation.token == progress.token)
        {
            self.progress_create = None;
        }
        if !progress.opened {
            return Ok(());
        }
        self.report_progress(
            progress.token,
            WorkDoneProgress::End(WorkDoneProgressEnd { message: None }),
        )
    }

    /// Send one progress report for one token.
    pub(super) fn report_progress(
        &mut self,
        token: NumberOrString,
        value: WorkDoneProgress,
    ) -> Result<()> {
        self.notify(
            notification::Progress::METHOD,
            ProgressParams {
                token,
                value: ProgressParamsValue::WorkDone(value),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::SourceJob;
    use crate::compiler::{SourceCompilation, SourceFailure};
    use crate::connection::tests::{messages, ready, ready_workspace};
    use crate::protocol::CheckProgress;
    use lsp_server::{Message, Notification, Response};
    use lsp_types::notification::{self, Notification as LspNotification};
    use serde_json::json;
    use std::ops::ControlFlow;

    #[test]
    fn progress_waits_for_creation_reply() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let written = messages(&mut connection);
        assert!(
            written
                .iter()
                .all(|message| progress_kind(message).is_none())
        );
        let Message::Request(create) = &written[0] else {
            panic!("progress creation request");
        };
        connection
            .response(Response::new_ok(create.id.clone(), ()))
            .unwrap();
        let written = messages(&mut connection);
        assert_eq!(
            written.iter().filter_map(progress_kind).collect::<Vec<_>>(),
            ["begin"]
        );
    }

    #[test]
    fn refused_progress_stays_silent() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let written = messages(&mut connection);
        let Message::Request(create) = &written[0] else {
            panic!("progress creation request");
        };
        connection
            .response(Response::new_err(
                create.id.clone(),
                -32603,
                "refused".into(),
            ))
            .unwrap();
        connection.end_progress().unwrap();
        assert!(
            written
                .iter()
                .all(|message| progress_kind(message).is_none())
        );
        assert!(messages(&mut connection).is_empty());
    }

    /// A report that ends before the client answered its creation releases that creation: the next
    /// report asks for its own, and the older answer reaches no report.
    #[test]
    fn unanswered_creation_does_not_block_the_next_report() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let written = messages(&mut connection);
        let Message::Request(superseded) = &written[0] else {
            panic!("progress creation request");
        };
        let superseded_id = superseded.id.clone();
        let superseded_token = superseded.params["token"].clone();
        connection.end_progress().unwrap();
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let written = messages(&mut connection);
        let Message::Request(created) = &written[0] else {
            panic!("the next report asks for its own creation: {written:?}");
        };
        assert_ne!(created.params["token"], superseded_token);
        assert!(
            written
                .iter()
                .all(|message| progress_kind(message).is_none())
        );
        // The answer to the released creation reaches no report.
        connection
            .response(Response::new_ok(superseded_id, ()))
            .unwrap();
        assert!(messages(&mut connection).is_empty());
        connection
            .response(Response::new_ok(created.id.clone(), ()))
            .unwrap();
        assert_eq!(
            messages(&mut connection)
                .iter()
                .filter_map(progress_kind)
                .collect::<Vec<_>>(),
            ["begin"]
        );
    }

    /// A creation the client leaves unanswered past its bound is given up with its report: nothing
    /// is asked twice, and the late answer reaches no report.
    #[test]
    fn unanswered_creation_gives_up_its_report() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let written = messages(&mut connection);
        let Message::Request(create) = &written[0] else {
            panic!("progress creation request");
        };
        connection
            .run_due_work(Instant::now() + PROGRESS_CREATION_TIMEOUT)
            .unwrap();
        assert!(connection.progress.is_none(), "the report is given up");
        assert!(
            messages(&mut connection).is_empty(),
            "nothing is asked twice"
        );
        connection
            .response(Response::new_ok(create.id.clone(), ()))
            .unwrap();
        assert!(messages(&mut connection).is_empty());
    }

    #[test]
    fn progress_cancellation_stops_matching_check() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.sources_changed(false).unwrap();
        connection.next_job(Instant::now()).unwrap();
        let token = connection.progress.as_ref().unwrap().token.clone();
        assert_eq!(
            connection
                .notification(Notification::new(
                    notification::WorkDoneProgressCancel::METHOD.into(),
                    json!({"token":"unrelated"}),
                ))
                .unwrap(),
            ControlFlow::Continue(())
        );
        assert!(!connection.checking.token().is_cancelled());
        assert_eq!(
            connection
                .notification(Notification::new(
                    notification::WorkDoneProgressCancel::METHOD.into(),
                    json!({"token":token}),
                ))
                .unwrap(),
            ControlFlow::Continue(())
        );
        assert!(connection.checking.token().is_cancelled());
        assert_eq!(connection.check_progress(), CheckProgress::NotChecked);
    }

    /// Which kind of `$/progress` report one message is, when it is one.
    fn progress_kind(message: &Message) -> Option<String> {
        let Message::Notification(notification) = message else {
            return None;
        };
        // `$/progress` is the one notification Tola sends that is not in the standard set, so its
        // kind is read from the body rather than from a constant.
        (notification.method == "$/progress")
            .then(|| serde_json::to_value(&notification.params).ok())
            .flatten()?
            .get("value")?
            .get("kind")?
            .as_str()
            .map(str::to_owned)
    }

    /// The first check requests its report at once and ends it when that check does.
    /// A check that is still queued is never a site that does not compile:
    /// an answer with no compiled site reads the progress to tell those two apart.
    #[test]
    fn cold_check_opens_and_ends_one_report() {
        let mut connection = ready(Vec::new());
        let workspace = ready_workspace(&mut connection);
        workspace.features.work_done_progress = true;
        connection.sources_changed(false).unwrap();
        let revision = connection.revision;
        assert_eq!(connection.check_progress(), CheckProgress::Checking);
        let now = Instant::now();
        assert!(matches!(
            connection.next_job(now),
            Some(SourceJob::Check(_))
        ));
        connection.run_due_work(now).unwrap();
        let written = messages(&mut connection);
        assert!(
            written
                .iter()
                .any(|message| matches!(message, Message::Request(request)
                if request.method == request::WorkDoneProgressCreate::METHOD)),
            "the report is created before it is reported: {written:?}"
        );
        let create = written
            .iter()
            .find_map(|message| match message {
                Message::Request(request)
                    if request.method == request::WorkDoneProgressCreate::METHOD =>
                {
                    Some(request.id.clone())
                }
                _ => None,
            })
            .unwrap();
        connection.response(Response::new_ok(create, ())).unwrap();
        let kinds: Vec<String> = messages(&mut connection)
            .iter()
            .filter_map(progress_kind)
            .collect();
        assert!(kinds.contains(&"begin".to_owned()), "{kinds:?}");
        connection
            .completed(SourceCompilation::Checked {
                checked_revision: revision,
                checked: Err(SourceFailure::Failed(anyhow::anyhow!("no site"))),
            })
            .unwrap();
        assert_eq!(connection.check_progress(), CheckProgress::Failed);
        let kinds: Vec<String> = messages(&mut connection)
            .iter()
            .filter_map(progress_kind)
            .collect();
        assert!(kinds.contains(&"end".to_owned()), "{kinds:?}");
    }
}
