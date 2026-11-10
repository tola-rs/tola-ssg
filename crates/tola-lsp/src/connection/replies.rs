//! The replies this connection writes: response construction, error mapping, and the shaping every
//! source reply passes through before its client-facing form.

use std::io::Write;

use anyhow::Result;
use lsp_server::{ErrorCode, RequestId, Response};
use tola_diagnostic::Diagnostic;

use crate::compiler::SourceFailure;
use crate::protocol::{self, SourceReply};

use super::{Connection, PendingRequest};

/// The reply pair for a request the client cancelled.
pub(super) const CANCELLED_ERROR: (ErrorCode, &str) =
    (ErrorCode::RequestCanceled, "request cancelled");

/// The reply pair for a request no step of Tola could answer.
const REQUEST_FAILED_ERROR: (ErrorCode, &str) = (
    ErrorCode::RequestFailed,
    "Tola could not answer this request",
);

/// The reply pair for a request whose own work panicked.
pub(super) const PANICKED_ERROR: (ErrorCode, &str) = (
    ErrorCode::InternalError,
    "Tola could not answer this request",
);

/// The reply pair for a request superseded by a newer source revision.
pub(super) const REVISION_CHANGED_ERROR: (ErrorCode, &str) =
    (ErrorCode::ContentModified, "source revision changed");

/// The reply pair for a request refused because the connection is shutting down.
pub(super) const SHUTDOWN_ERROR: (ErrorCode, &str) = (
    ErrorCode::RequestCanceled,
    "language server is shutting down",
);

pub(super) fn error_response(id: RequestId, (code, message): (ErrorCode, &str)) -> Response {
    Response::new_err(id, code as i32, message.into())
}

/// Keep the fixes a code action request already earned beside what the checked world adds.
pub(super) fn merge_code_actions(
    request: &PendingRequest,
    response: &mut Result<SourceReply, SourceFailure>,
) {
    let Some(narrowing) = request.narrowing.as_ref() else {
        return;
    };
    let Ok(SourceReply::CodeActions(semantic)) = response else {
        return;
    };
    let mut actions = narrowing.clone();
    actions.extend(semantic.take().unwrap_or_default());
    *semantic = (!actions.is_empty()).then_some(actions);
}

pub(super) fn failed_response(id: RequestId, failure: SourceFailure) -> Response {
    match failure {
        SourceFailure::Cancelled => error_response(id, CANCELLED_ERROR),
        SourceFailure::Panicked => error_response(id, PANICKED_ERROR),
        SourceFailure::InvalidName(error) => {
            let message = error.to_string();
            crate::protocol::with_data(
                Response::new_err(id, ErrorCode::InvalidParams as i32, message.clone()),
                serde_json::json!({ "kind": error.kind(), "message": message }),
            )
        }
        SourceFailure::Failed(error) => {
            tracing::error!(error = ?error, "a source request failed");
            error_response(id, REQUEST_FAILED_ERROR)
        }
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn source_reply(&mut self, id: RequestId, reply: SourceReply) -> Result<()> {
        let mut reply = self.shaped_reply(reply);
        let workspace = self.workspace();
        workspace.features.reply(&mut reply);
        self.reply(Response::new_ok(id, reply))
    }

    /// The shaping every source reply passes through before its client-facing form.
    fn shaped_reply(&mut self, mut reply: SourceReply) -> SourceReply {
        // A preview lens names the route it opens; where this connection knows where the site is
        // served, the same lens names the page's own address too, which a client running no
        // command reads.
        if let SourceReply::CodeLenses(Some(lenses)) = &mut reply {
            for lens in lenses {
                let Some(command) = &lens.command else {
                    continue;
                };
                if let Some(route) = protocol::preview_route(command) {
                    let url = self.preview_url(&route);
                    lens.data = Some(protocol::preview_lens_data(&route, url.as_deref()));
                }
            }
        }
        if let SourceReply::Rename(Some(edit)) = &mut reply {
            self.version_document_edits(edit);
        }
        if let SourceReply::CodeActions(Some(actions)) = &mut reply {
            for action in actions {
                let lsp_types::CodeActionOrCommand::CodeAction(action) = action else {
                    continue;
                };
                let Some(edit) = &mut action.edit else {
                    continue;
                };
                self.version_document_edits(edit);
            }
        }
        reply
    }

    /// Version one workspace edit's document edits, so an editor applies them to the buffer it
    /// asked about.
    ///
    /// Only a client that declared `documentChanges` reads this shape; the capability projection
    /// degrades it back to `changes` for every other client. An edit that touches no document the
    /// client holds keeps the plain shape: a version is the client's own record of a buffer.
    fn version_document_edits(&self, edit: &mut lsp_types::WorkspaceEdit) {
        if edit.document_changes.is_some() {
            return;
        }
        let Some(changes) = edit.changes.as_ref() else {
            return;
        };
        if !changes
            .keys()
            .any(|uri| self.open.version(uri.as_str()).is_some())
        {
            return;
        }
        let mut changes = edit
            .changes
            .take()
            .expect("owned document edits")
            .into_iter()
            .collect::<Vec<_>>();
        changes.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
        edit.document_changes = Some(lsp_types::DocumentChanges::Edits(
            changes
                .into_iter()
                .map(|(uri, edits)| lsp_types::TextDocumentEdit {
                    text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                        version: self.open.version(uri.as_str()),
                        uri,
                    },
                    edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
                })
                .collect(),
        ));
    }

    pub(super) fn reject(&mut self, id: RequestId, code: ErrorCode, message: &str) -> Result<()> {
        self.reply(error_response(id, (code, message)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{ready, response};
    use lsp_types::Uri;

    #[test]
    fn conflicting_buffers_explain_recovery() {
        for failure in [
            anyhow::Error::new(crate::analysis::NameQueryError::ConflictingSourceText),
            anyhow::Error::new(crate::analysis::NameQueryError::ConflictingSourceText)
                .context("a source query failed"),
        ] {
            let response = failed_response(7.into(), SourceFailure::of(failure));
            let error = response.response_result.unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidParams as i32);
            assert_eq!(error.data.unwrap()["kind"], "conflictingSourceText");
            assert!(error.message.contains("save or close"));
        }
    }

    #[test]
    fn entity_edits_keep_open_versions() {
        use crate::connection::tests::{open_document, ready_at, ready_workspace};
        let temporary = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), temporary.path().to_owned());
        let capabilities = serde_json::from_value(serde_json::json!({
            "workspace": {"workspaceEdit": {"documentChanges": true}}
        }))
        .unwrap();
        ready_workspace(&mut connection).features =
            crate::capabilities::ClientFeatures::new(&capabilities);
        let definition =
            crate::uri::from_file_path(&temporary.path().join("definitions.typ")).unwrap();
        let reference = crate::uri::from_file_path(&temporary.path().join("author.typ")).unwrap();
        open_document(&mut connection, &definition, 7, "#define(\"sku:α\")");
        open_document(&mut connection, &reference, 3, "#link(\"sku:α\")");
        let edits = [definition.clone(), reference.clone()]
            .into_iter()
            .map(|uri| {
                (
                    uri,
                    vec![lsp_types::TextEdit {
                        range: lsp_types::Range::default(),
                        new_text: "\"sku:β\"".into(),
                    }],
                )
            })
            .collect();
        connection
            .source_reply(
                19.into(),
                SourceReply::Rename(Some(lsp_types::WorkspaceEdit {
                    changes: Some(edits),
                    ..Default::default()
                })),
            )
            .unwrap();
        let result = response(&mut connection).response_result.unwrap();
        assert!(result.get("changes").is_none());
        let changes = result["documentChanges"].as_array().unwrap();
        assert_eq!(changes.len(), 2);
        for (uri, version) in [(definition, 7), (reference, 3)] {
            let change = changes
                .iter()
                .find(|change| change["textDocument"]["uri"] == uri.as_str())
                .unwrap();
            assert_eq!(change["textDocument"]["version"], version);
            assert_eq!(change["edits"][0]["newText"], "\"sku:β\"");
        }
    }

    /// A preview lens names the route it opens, and the address this connection knows the page is
    /// served at: a client that runs no command of its own still reads the page it is about.
    #[test]
    fn preview_lens_carries_its_url() {
        let mut connection = ready(Vec::new());
        connection.default_origin = Some("http://localhost:4321".to_owned());
        let uri: Uri = "file:///site/page.typ".parse().unwrap();
        let lens = |route: Option<&str>| {
            let mut arguments = vec![serde_json::Value::String(uri.as_str().to_owned())];
            if let Some(route) = route {
                arguments.push(serde_json::Value::String(route.to_owned()));
            }
            lsp_types::CodeLens {
                range: lsp_types::Range::default(),
                command: Some(lsp_types::Command {
                    title: "Save and preview".to_owned(),
                    command: protocol::PREVIEW_COMMAND.to_owned(),
                    arguments: Some(arguments),
                }),
                data: None,
            }
        };
        connection
            .source_reply(
                1.into(),
                SourceReply::CodeLenses(Some(vec![lens(Some("/page/")), lens(None)])),
            )
            .unwrap();
        let response = response(&mut connection);
        let lenses: Vec<lsp_types::CodeLens> =
            serde_json::from_value(response.response_result.unwrap()).unwrap();
        assert_eq!(
            lenses[0].data,
            Some(serde_json::json!({ "route": "/page/", "url": "http://localhost:4321/page/" }))
        );
        // A lens that names no route offers a choice of pages, and this connection names no page
        // for it.
        assert_eq!(lenses[1].data, None);
    }
}
