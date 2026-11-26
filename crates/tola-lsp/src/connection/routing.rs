//! Client message dispatch: request admission/refusal, notifications, and cancellations.

use std::io::Write;
use std::ops::ControlFlow;

use anyhow::Result;
use lsp_server::{ErrorCode, Notification, Request, RequestId, Response};
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::{LogMessageParams, MessageType, NumberOrString, Uri};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_diagnostic::Diagnostic;

use crate::protocol::{CheckProgress, ClientRequest};

use super::local_requests::is_untitled;
use super::replies::{CANCELLED_ERROR, REVISION_CHANGED_ERROR, SHUTDOWN_ERROR, error_response};
use super::{Connection, PendingRequest, Phase};

const MAX_SOURCE_REQUESTS: usize = 64;

const SERVER_BUSY_ERROR_CODE: i32 = -32000;

#[derive(Debug, thiserror::Error)]
pub(super) enum ConnectionError {
    #[error("LSP exit received before shutdown")]
    ExitBeforeShutdown,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub(super) enum InvalidNotification {
    FileChange,
    CancelRequest,
    Configuration,
}

impl InvalidNotification {
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

pub(super) struct SourceAdmission {
    pub(super) serial: u64,
    pub(super) cancellation: BuildCancellation,
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn request(&mut self, req: Request) -> Result<()> {
        if let Some((code, msg)) = self.phase.request_error(&req.method) {
            return self.reject(req.id, code, msg);
        }
        // Reject duplicate pending IDs before any handler runs so replies stay unambiguous.
        if self.requests.contains_key(&req.id) {
            return self.reject(
                req.id,
                ErrorCode::InvalidRequest,
                "request id is already pending",
            );
        }
        let (id, req) = match ClientRequest::decode(req) {
            Ok(req) => req,
            Err(resp) => return self.reply(*resp),
        };
        match req {
            ClientRequest::Initialize(params) => self.initialize(id, *params),
            ClientRequest::Shutdown => {
                self.cancel_pending_work(SHUTDOWN_ERROR)?;
                self.running_check = None;
                self.unwatch_sources()?;
                self.phase = Phase::Shutdown;
                self.reply(Response::new_ok(id, ()))
            }
            ClientRequest::Query(query) => self.query(id, query),
            ClientRequest::Formatting(params) => self.formatting(id, params),
            ClientRequest::RangeFormatting(params) => self.range_formatting(id, params),
            ClientRequest::Folding(params) => self.folding(id, params),
            ClientRequest::Symbols(params) => self.symbols(id, params),
            ClientRequest::Links(params) => self.links(id, params),
            ClientRequest::Selection(params) => self.selection(id, params),
            ClientRequest::Tokens(params) => self.tokens(id, params),
            ClientRequest::Route(params) => self.route(id, params),
            ClientRequest::CodeLenses(params) => self.code_lens(id, params),
            ClientRequest::Rename(params) => self.rename(id, params),
            ClientRequest::WorkspaceSymbols(params) => self.workspace_symbols(id, params),
            ClientRequest::Enter(params) => self.enter(id, params),
            ClientRequest::TokensDelta(params) => self.tokens_delta(id, params),
            ClientRequest::TokensRange(params) => self.tokens_range(id, params),
            ClientRequest::ExecuteCommand(params) => self.command(id, params),
            ClientRequest::Diagnostic(params) => self.diagnostic(id, params),
            ClientRequest::WorkspaceDiagnostic(params) => self.workspace_diagnostic(id, params),
            ClientRequest::PackageSource(params) => self.package_source(id, params),
            ClientRequest::RouteIndex => self.route_index(id),
            ClientRequest::PrepareCallHierarchy(params) => self.prepare_call_hierarchy(id, params),
            ClientRequest::IncomingCalls(params) => self.incoming_calls(id, params),
            ClientRequest::OutgoingCalls(params) => self.outgoing_calls(id, params),
        }
    }

    pub(super) fn admit(&mut self, id: &RequestId) -> Result<Option<SourceAdmission>> {
        if self.requests.len() >= MAX_SOURCE_REQUESTS {
            self.reply(Response::new_err(
                id.clone(),
                SERVER_BUSY_ERROR_CODE,
                "too many pending source requests".into(),
            ))?;
            return Ok(None);
        }
        self.serial += 1;
        let canceller = BuildCanceller::new();
        let cancel = canceller.token();
        self.requests.insert(
            id.clone(),
            PendingRequest {
                serial: self.serial,
                canceller,
                document_revision: None,
                narrowing: None,
                waiting: None,
            },
        );
        Ok(Some(SourceAdmission {
            serial: self.serial,
            cancellation: cancel,
        }))
    }

    pub(super) fn notification(
        &mut self,
        msg: Notification,
    ) -> std::result::Result<ControlFlow<()>, ConnectionError> {
        if msg.method == notification::Exit::METHOD {
            if !self.is_shutdown() {
                return Err(ConnectionError::ExitBeforeShutdown);
            }
            return Ok(ControlFlow::Break(()));
        }
        if matches!(self.phase, Phase::Uninitialized | Phase::Shutdown) {
            return Ok(ControlFlow::Continue(()));
        }
        if msg.method == notification::Cancel::METHOD {
            self.cancel_request(msg)?;
            return Ok(ControlFlow::Continue(()));
        }
        if msg.method == notification::WorkDoneProgressCancel::METHOD {
            if let Ok(params) = msg.extract::<lsp_types::WorkDoneProgressCancelParams>(
                notification::WorkDoneProgressCancel::METHOD,
            ) && self
                .progress
                .as_ref()
                .is_some_and(|p| p.token == params.token && p.revision == self.revision)
            {
                self.checking.cancel();
                self.check = None;
                self.running_check = None;
                self.end_progress()?;
                self.report_source_check(CheckProgress::NotChecked)?;
            }
            return Ok(ControlFlow::Continue(()));
        }
        if msg.method == notification::Initialized::METHOD {
            self.initialized()?;
            return Ok(ControlFlow::Continue(()));
        }
        // Drop document notifications arriving between `initialize` reply and `initialized`.
        if matches!(self.phase, Phase::Initializing(_)) {
            return Ok(ControlFlow::Continue(()));
        }
        if msg.method == notification::DidChangeConfiguration::METHOD {
            self.configured(msg)?;
            return Ok(ControlFlow::Continue(()));
        }
        if msg.method == notification::DidChangeWatchedFiles::METHOD
            && !self.watched_change_matters(&msg)
        {
            return Ok(ControlFlow::Continue(()));
        }
        self.apply_document_notification(msg)?;
        Ok(ControlFlow::Continue(()))
    }

    fn apply_document_notification(&mut self, msg: Notification) -> Result<()> {
        let uri = msg
            .params
            .get("textDocument")
            .and_then(|doc| doc.get("uri"))
            .and_then(serde_json::Value::as_str)
            .and_then(|uri| uri.parse::<Uri>().ok());
        let unnamed = uri.as_ref().is_some_and(is_untitled);
        let opened = msg.method == notification::DidOpenTextDocument::METHOD;
        let closed = msg.method == notification::DidCloseTextDocument::METHOD;
        let typing = msg.method == notification::DidChangeTextDocument::METHOD;
        match self.open.apply(msg) {
            Ok(changed) => {
                if let Some(uri) = &uri {
                    if closed {
                        self.tokens.remove(uri);
                    }
                    if opened
                        && changed
                        && let Phase::Ready(ws) = &mut self.phase
                    {
                        ws.diagnostics.invalidate_version(uri);
                    }
                }
                if changed && !unnamed {
                    self.sources_changed(typing)?;
                } else if changed && let Some(uri) = &uri {
                    self.cancel_document_requests(uri)?;
                }
            }
            Err(_) => self.log_invalid_notification(InvalidNotification::FileChange)?,
        }
        Ok(())
    }

    fn cancel_request(&mut self, msg: Notification) -> Result<()> {
        let params = match msg.extract::<lsp_types::CancelParams>(notification::Cancel::METHOD) {
            Ok(params) => params,
            Err(_) => return self.log_invalid_notification(InvalidNotification::CancelRequest),
        };
        let id = match params.id {
            NumberOrString::Number(id) => RequestId::from(id),
            NumberOrString::String(id) => RequestId::from(id),
        };
        if let Some(req) = self.requests.remove(&id) {
            req.canceller.cancel();
            if let Some(waiting) = req.waiting
                && self.waiting_code_actions(waiting.revision).is_empty()
            {
                self.cancel_selection();
            }
            self.discard_cancelled_jobs();
            self.reply(error_response(id, CANCELLED_ERROR))?;
        }
        Ok(())
    }

    pub(super) fn cancel_requests(&mut self, refusal: (ErrorCode, &str)) -> Result<()> {
        // Cancel all jobs before sending replies in case response delivery fails.
        for req in self.requests.values() {
            req.canceller.cancel();
        }
        self.discard_cancelled_jobs();
        for id in std::mem::take(&mut self.requests).into_keys() {
            self.reply(error_response(id, refusal))?;
        }
        Ok(())
    }

    fn cancel_document_requests(&mut self, uri: &Uri) -> Result<()> {
        let ids = self
            .requests
            .iter()
            .filter_map(|(id, req)| {
                req.document_revision
                    .as_ref()
                    .filter(|(doc, _)| doc == uri)
                    .map(|_| id.clone())
            })
            .collect::<Vec<_>>();
        for id in &ids {
            self.requests
                .remove(id)
                .expect("owned document request")
                .canceller
                .cancel();
        }
        self.discard_cancelled_jobs();
        for id in ids {
            self.reply(error_response(id, REVISION_CHANGED_ERROR))?;
        }
        Ok(())
    }

    pub(super) fn discard_cancelled_jobs(&mut self) {
        self.analyses
            .retain(|req| !req.sources.cancellation.is_cancelled());
        self.queries.retain(|job| !job.is_cancelled());
    }

    pub(super) fn log_invalid_notification(&mut self, kind: InvalidNotification) -> Result<()> {
        self.notify(
            notification::LogMessage::METHOD,
            LogMessageParams {
                typ: MessageType::ERROR,
                message: kind.message().to_owned(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{hover_query, ready, rejection, uninitialized};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;

    fn completion_request(id: i32, uri: &Uri, marked: &str) -> Request {
        let src = tola_typst::typst::syntax::Source::detached(marked.replace('|', ""));
        let cursor = marked.find('|').unwrap();
        let pos = crate::position::utf16_range(src.lines(), cursor..cursor)
            .unwrap()
            .start;
        Request::new(
            id.into(),
            request::Completion::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str()},"position":pos}),
        )
    }

    #[test]
    fn control_requests_unblocked_by_completion() {
        use crate::connection::tests::{open_document, response};
        let mut conn = ready(Vec::new());
        let uri =
            crate::uri::from_file_path(&std::env::current_dir().unwrap().join("completion.typ"))
                .unwrap();
        let marked = "#let value = (available: 1)\n#value.|";
        open_document(&mut conn, &uri, 1, &marked.replace('|', ""));
        conn.request(completion_request(8, &uri, marked)).unwrap();
        let prev = conn.next_analysis().unwrap();
        let cancel = prev.sources.cancellation.clone();
        assert!(conn.writer.is_empty());
        conn.request(Request::new(
            9.into(),
            "textDocument/foldingRange".into(),
            json!({"textDocument":{"uri":uri.as_str()}}),
        ))
        .unwrap();
        let folding = response(&mut conn);
        assert_eq!(folding.id, 9.into());
        assert!(folding.response_result.is_ok());
        let _ = conn
            .notification(Notification::new("$/cancelRequest".into(), json!({"id":8})))
            .unwrap();
        assert_eq!(
            response(&mut conn).response_result.unwrap_err().code,
            ErrorCode::RequestCanceled as i32
        );
        assert!(cancel.is_cancelled());
        conn.request(completion_request(8, &uri, marked)).unwrap();
        let curr = conn.next_analysis().unwrap();
        let mut disk = crate::analysis::DiskSources::default();
        let mut graphs = crate::analysis::GraphCache::default();
        conn.completed(crate::compiler::analyze(&mut disk, &mut graphs, prev))
            .unwrap();
        assert!(conn.writer.is_empty());
        conn.completed(crate::compiler::analyze(&mut disk, &mut graphs, curr))
            .unwrap();
        let items = response(&mut conn).response_result.unwrap();
        assert!(
            items
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "available")
        );
    }

    #[test]
    fn duplicate_request_id_rejected() {
        let mut conn = ready(Vec::new());
        let pending = RequestId::from(1);
        conn.query(pending.clone(), hover_query()).unwrap();
        assert!(conn.requests.contains_key(&pending));
        conn.request(Request::new(
            pending.clone(),
            request::Formatting::METHOD.into(),
            json!({
                "textDocument": {"uri": "file:///site/document.typ"},
                "options": {"tabSize": 2, "insertSpaces": true},
            }),
        ))
        .unwrap();
        assert_eq!(
            rejection(&mut conn, &pending),
            ErrorCode::InvalidRequest as i32
        );
        assert!(conn.requests.contains_key(&pending));
    }

    #[test]
    fn exit_requires_shutdown() {
        let mut conn = uninitialized();
        assert!(
            conn.notification(Notification::new(
                notification::Exit::METHOD.into(),
                json!({}),
            ))
            .is_err()
        );
    }
}
