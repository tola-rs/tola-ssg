//! The work-done report a running check opens: when it is due, the client's creation answer that
//! opens it, and the reports that give it up.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use lsp_server::Response;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressCreateParams, WorkDoneProgressEnd,
};
use tola_diagnostic::Diagnostic;

use super::{Connection, PendingCreation, Phase, Progress};

/// How long a check may run before the client is told that it is running.
///
/// A check that finishes inside this window reports nothing: the report would arrive after the
/// author stopped waiting for it, and a client showing one has nothing to show.
const PROGRESS_DELAY: Duration = Duration::from_millis(300);

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
        let serial = self.next_identifier();
        self.progress = Some(Progress {
            revision,
            token: NumberOrString::String(format!("tola-check-{serial}")),
            due,
            opened: false,
        });
    }

    /// Answer the client's reply to one report creation; `false` when it answers something else.
    ///
    /// A client that refused one report is never sent another: nothing Tola does needs a report, and
    /// repeating a refused request is noise.
    pub(super) fn progress_created(&mut self, response: &Response) -> Result<bool> {
        let Some(creation) = self
            .progress_create
            .take_if(|creation| creation.id == response.id)
        else {
            return Ok(false);
        };
        self.progress_agreed = response.response_result.is_ok();
        if !self.progress_agreed {
            self.progress = None;
        } else if let Some(progress) = self.progress.as_mut()
            && progress.token == creation.token
        {
            progress.opened = true;
            self.report_progress(
                creation.token,
                WorkDoneProgress::Begin(WorkDoneProgressBegin {
                    title: PROGRESS_TITLE.to_owned(),
                    cancellable: Some(true),
                    message: None,
                    percentage: None,
                }),
            )?;
        }
        Ok(true)
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
        if self.progress_create.is_some() {
            return Ok(());
        }
        let id = self.send_request(
            request::WorkDoneProgressCreate::METHOD,
            WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        )?;
        self.progress_create = Some(PendingCreation { id, token });
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
    use crate::connection::tests::{messages, ready, ready_workspace};
    use lsp_server::{Message, Response};

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
    fn superseded_progress_stays_closed() {
        let mut connection = ready(Vec::new());
        ready_workspace(&mut connection).features.work_done_progress = true;
        connection.begin_progress(Instant::now());
        connection.run_due_work(Instant::now()).unwrap();
        let Message::Request(create) = messages(&mut connection).remove(0) else {
            panic!("progress creation request");
        };
        connection.sources_changed(false).unwrap();
        connection
            .response(Response::new_ok(create.id, ()))
            .unwrap();
        assert!(messages(&mut connection).is_empty());
        assert!(connection.progress.is_none());
        assert!(connection.progress_create.is_none());
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
        assert!(messages(&mut connection).is_empty());
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
}
