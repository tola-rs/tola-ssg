//! Client message dispatch: request admission and refusal, notifications, and cancellations.

use std::io::Write;
use std::ops::ControlFlow;

use anyhow::{Context, Result, bail};
use lsp_server::{ErrorCode, Notification, Request, RequestId, Response};
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::{LogMessageParams, MessageType, NumberOrString, Uri};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::diagnostic::Diagnostic;

use crate::protocol::{CheckProgress, ClientRequest};

use super::local_requests::is_untitled;
use super::replies::{CANCELLED_ERROR, error_response};
use super::{Connection, PendingRequest, Phase};

const MAX_SOURCE_REQUESTS: usize = 64;

const SERVER_BUSY_ERROR_CODE: i32 = -32000;

/// One notification the connection could not read, named in the editor's log.
pub(super) enum InvalidNotification {
    FileChange,
    CancelRequest,
    Configuration,
}

impl InvalidNotification {
    /// One sentence naming the notification and what the author does next; an editor log entry
    /// has no separate help field for the action.
    fn message(self) -> &'static str {
        match self {
            Self::FileChange => {
                "Tola could not apply the editor's file change; reload the document"
            }
            Self::CancelRequest => {
                "Tola could not read the editor's cancel request; the request it named still runs"
            }
            Self::Configuration => {
                "Tola could not read the editor's settings; the current settings stay"
            }
        }
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn request(&mut self, request: Request) -> Result<()> {
        if let Some((code, message)) = self.phase.request_error(&request.method) {
            return self.reject(request.id, code, message);
        }
        // Two replies for one identifier would leave the client matching the wrong one, so an id a
        // pending request holds is refused here, before any handler answers.
        if self.requests.contains_key(&request.id) {
            return self.reject(
                request.id,
                ErrorCode::InvalidRequest,
                "request id is already pending",
            );
        }
        let (id, request) = match ClientRequest::decode(request) {
            Ok(request) => request,
            Err(response) => return self.reply(*response),
        };
        match request {
            ClientRequest::Initialize(parameters) => self.initialize(id, *parameters),
            ClientRequest::Shutdown => {
                self.checking.cancel();
                self.cancel_selection();
                self.cancel_requests(
                    ErrorCode::RequestCanceled,
                    "language server is shutting down",
                )?;
                self.queries.clear();
                self.analyses.clear();
                self.check = None;
                self.running_check = None;
                self.end_progress()?;
                self.unwatch_sources()?;
                self.phase = Phase::Shutdown;
                self.reply(Response::new_ok(id, ()))
            }
            ClientRequest::Query(query) => self.query(id, query),
            ClientRequest::Formatting(parameters) => self.formatting(id, parameters),
            ClientRequest::RangeFormatting(parameters) => self.range_formatting(id, parameters),
            ClientRequest::Folding(parameters) => self.folding(id, parameters),
            ClientRequest::Symbols(parameters) => self.symbols(id, parameters),
            ClientRequest::Links(parameters) => self.links(id, parameters),
            ClientRequest::Selection(parameters) => self.selection(id, parameters),
            ClientRequest::Tokens(parameters) => self.tokens(id, parameters),
            ClientRequest::Route(parameters) => self.route(id, parameters),
            ClientRequest::CodeLenses(parameters) => self.code_lens(id, parameters),
            ClientRequest::Rename(parameters) => self.rename(id, parameters),
            ClientRequest::WorkspaceSymbols(parameters) => self.workspace_symbols(id, parameters),
            ClientRequest::Enter(parameters) => self.enter(id, parameters),
            ClientRequest::TokensDelta(parameters) => self.tokens_delta(id, parameters),
            ClientRequest::TokensRange(parameters) => self.tokens_range(id, parameters),
            ClientRequest::ExecuteCommand(parameters) => self.command(id, parameters),
            ClientRequest::Diagnostic(parameters) => self.diagnostic(id, parameters),
            ClientRequest::PackageSource(parameters) => self.package_source(id, parameters),
            ClientRequest::RouteIndex => self.route_index(id),
            ClientRequest::PrepareCallHierarchy(parameters) => {
                self.prepare_call_hierarchy(id, parameters)
            }
            ClientRequest::IncomingCalls(parameters) => self.incoming_calls(id, parameters),
            ClientRequest::OutgoingCalls(parameters) => self.outgoing_calls(id, parameters),
        }
    }

    /// Register one source request, or reply with its rejection and return `None`.
    pub(super) fn admit(&mut self, id: &RequestId) -> Result<Option<(u64, BuildCancellation)>> {
        if self.requests.len() >= MAX_SOURCE_REQUESTS {
            self.reply(Response::new_err(
                id.clone(),
                SERVER_BUSY_ERROR_CODE,
                "too many pending source requests".into(),
            ))?;
            return Ok(None);
        }
        self.serial = self
            .serial
            .checked_add(1)
            .context("source request counter exhausted")?;
        let canceller = BuildCanceller::new();
        let cancellation = canceller.token();
        self.requests.insert(
            id.clone(),
            PendingRequest {
                serial: self.serial,
                canceller,
                narrowing: None,
                waiting: None,
            },
        );
        Ok(Some((self.serial, cancellation)))
    }

    pub(super) fn notification(&mut self, message: Notification) -> Result<ControlFlow<()>> {
        if message.method == notification::Exit::METHOD {
            if !self.is_shutdown() {
                bail!("LSP exit received before shutdown");
            }
            return Ok(ControlFlow::Break(()));
        }
        if matches!(self.phase, Phase::Uninitialized | Phase::Shutdown) {
            return Ok(ControlFlow::Continue(()));
        }
        if message.method == notification::Cancel::METHOD {
            self.cancel_request(message)?;
            return Ok(ControlFlow::Continue(()));
        }
        if message.method == notification::WorkDoneProgressCancel::METHOD {
            if let Ok(parameters) = message.extract::<lsp_types::WorkDoneProgressCancelParams>(
                notification::WorkDoneProgressCancel::METHOD,
            ) && self.progress.as_ref().is_some_and(|progress| {
                progress.token == parameters.token && progress.revision == self.revision
            }) {
                self.checking.cancel();
                self.check = None;
                self.check_due = None;
                self.running_check = None;
                self.end_progress()?;
                self.report_source_check(CheckProgress::NotChecked)?;
            }
            return Ok(ControlFlow::Continue(()));
        }
        if message.method == notification::Initialized::METHOD {
            self.initialized()?;
            return Ok(ControlFlow::Continue(()));
        }
        // A source notification between the `initialize` reply and `initialized` has no ready
        // workspace to change, so it is dropped like one that arrives before `initialize`; only
        // `initialized`, called above, ends the window.
        if matches!(self.phase, Phase::Initializing(_)) {
            return Ok(ControlFlow::Continue(()));
        }
        // A settings change is not a source change: it re-reads what the client says about this
        // connection — the formatter, when a check runs, where the site is served — and leaves the
        // site's revision, its requests, and its held compilation as they are.
        if message.method == notification::DidChangeConfiguration::METHOD {
            self.configured(message)?;
            return Ok(ControlFlow::Continue(()));
        }
        // A changed file no check read, and no later check can read as a source, a configuration,
        // or a bibliography with its style sheet, changes nothing this connection answers;
        // dropping the notification here keeps it from costing a revision and a site-wide check.
        if message.method == notification::DidChangeWatchedFiles::METHOD
            && !self.watched_change_matters(&message)
        {
            return Ok(ControlFlow::Continue(()));
        }
        let uri = message
            .params
            .get("textDocument")
            .and_then(|document| document.get("uri"))
            .and_then(serde_json::Value::as_str)
            .and_then(|uri| uri.parse::<Uri>().ok());
        let unnamed = uri.as_ref().is_some_and(is_untitled);
        let closed = message.method == notification::DidCloseTextDocument::METHOD;
        let typing = message.method == notification::DidChangeTextDocument::METHOD;
        match self.open.apply(message) {
            Ok(changed) => {
                if closed && let Some(uri) = uri {
                    self.tokens.remove(&uri);
                }
                if changed && !unnamed {
                    self.sources_changed(typing)?;
                }
            }
            Err(_) => self.log_invalid_notification(InvalidNotification::FileChange)?,
        }
        Ok(ControlFlow::Continue(()))
    }

    fn cancel_request(&mut self, message: Notification) -> Result<()> {
        let parameters =
            match message.extract::<lsp_types::CancelParams>(notification::Cancel::METHOD) {
                Ok(parameters) => parameters,
                Err(_) => return self.log_invalid_notification(InvalidNotification::CancelRequest),
            };
        let id = match parameters.id {
            NumberOrString::Number(id) => RequestId::from(id),
            NumberOrString::String(id) => RequestId::from(id),
        };
        if let Some(request) = self.requests.remove(&id) {
            request.canceller.cancel();
            if let Some(waiting) = request.waiting
                && self.waiting_code_actions(waiting.revision).is_empty()
            {
                self.cancel_selection();
            }
            self.reply(error_response(id, CANCELLED_ERROR))?;
        }
        Ok(())
    }

    pub(super) fn cancel_requests(&mut self, code: ErrorCode, message: &str) -> Result<()> {
        // Cancel every job before writing: a disconnected client must not leave
        // a compiler running after response delivery fails partway through.
        for request in self.requests.values() {
            request.canceller.cancel();
        }
        for (id, _) in std::mem::take(&mut self.requests) {
            self.reject(id, code, message)?;
        }
        Ok(())
    }

    /// Report one notification Tola could not apply to the author, through the editor's log.
    ///
    /// A notification has no reply, so this message is the only report the editor receives. The
    /// protocol error stays unnamed here: what the author acts on is the notification their editor
    /// sent, not the wording of the parse failure.
    pub(super) fn log_invalid_notification(
        &mut self,
        notification: InvalidNotification,
    ) -> Result<()> {
        self.notify(
            notification::LogMessage::METHOD,
            LogMessageParams {
                typ: MessageType::ERROR,
                message: notification.message().to_owned(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::SourceJob;
    use crate::connection::tests::{hover_query, ready, rejection, uninitialized};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::time::Instant;

    /// A second request reusing an identifier whose request is still pending would give the client
    /// two replies for one id, so the dispatch boundary refuses it before any lane answers.
    #[test]
    fn duplicate_request_ids_are_rejected() {
        let mut connection = ready(Vec::new());
        let pending = RequestId::from(1);
        connection.query(pending.clone(), hover_query()).unwrap();
        assert!(connection.requests.contains_key(&pending));
        connection
            .request(Request::new(
                pending.clone(),
                request::Formatting::METHOD.into(),
                json!({
                    "textDocument": {"uri": "file:///site/document.typ"},
                    "options": {"tabSize": 2, "insertSpaces": true},
                }),
            ))
            .unwrap();
        assert_eq!(
            rejection(&mut connection, &pending),
            ErrorCode::InvalidRequest as i32
        );
        assert!(
            connection.requests.contains_key(&pending),
            "the admitted request stands"
        );
    }

    #[test]
    fn full_request_queue_is_rejected() {
        let mut connection = ready(Vec::new());
        for serial in 0..MAX_SOURCE_REQUESTS as u64 {
            connection.requests.insert(
                RequestId::from(serial as i32),
                PendingRequest {
                    serial,
                    canceller: BuildCanceller::new(),
                    narrowing: None,
                    waiting: None,
                },
            );
        }
        let full = RequestId::from(1_000);
        connection.query(full.clone(), hover_query()).unwrap();
        assert_eq!(rejection(&mut connection, &full), SERVER_BUSY_ERROR_CODE);
    }

    #[test]
    fn shutdown_rejects_new_requests() {
        let mut connection = ready(Vec::new());
        connection
            .request(Request::new(
                2.into(),
                request::Shutdown::METHOD.into(),
                serde_json::Value::Null,
            ))
            .unwrap();
        let after = RequestId::from(3);
        connection
            .request(Request::new(
                after.clone(),
                request::HoverRequest::METHOD.into(),
                serde_json::json!({}),
            ))
            .unwrap();
        assert_eq!(
            rejection(&mut connection, &after),
            ErrorCode::InvalidRequest as i32
        );
    }

    #[test]
    fn exit_requires_shutdown() {
        let mut connection = uninitialized();
        assert!(
            connection
                .notification(Notification::new(
                    notification::Exit::METHOD.into(),
                    json!({}),
                ))
                .is_err()
        );
    }

    #[test]
    fn failed_cancel_still_cancels_jobs() {
        struct Disconnected;
        impl Write for Disconnected {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut connection = ready(Disconnected);
        for id in [1, 2] {
            connection
                .query(RequestId::from(id), hover_query())
                .unwrap();
        }
        assert!(
            matches!(
                connection.next_job(Instant::now()),
                Some(SourceJob::Query(_))
            ),
            "both queries are admitted before the client disappears"
        );
        assert!(
            connection
                .cancel_requests(ErrorCode::RequestCanceled, "closing")
                .is_err()
        );
        assert!(
            connection.next_job(Instant::now()).is_none(),
            "a cancelled job must not reach the compiler"
        );
    }
}
