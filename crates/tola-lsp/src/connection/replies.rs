//! Response construction, failure mapping, and client-facing reply shaping.

use std::io::Write;

use anyhow::Result;
use lsp_server::{ErrorCode, RequestId, Response};
use tola_diagnostic::Diagnostic;

use crate::compiler::SourceFailure;
use crate::protocol::{self, SourceReply};

use super::{Connection, PendingRequest};

pub(super) const CANCELLED_ERROR: (ErrorCode, &str) =
    (ErrorCode::RequestCanceled, "request cancelled");

const REQUEST_FAILED_ERROR: (ErrorCode, &str) = (
    ErrorCode::RequestFailed,
    "Tola could not answer this request",
);

pub(super) const PANICKED_ERROR: (ErrorCode, &str) = (
    ErrorCode::InternalError,
    "Tola could not answer this request",
);

pub(super) const REVISION_CHANGED_ERROR: (ErrorCode, &str) =
    (ErrorCode::ContentModified, "source revision changed");

pub(super) const SHUTDOWN_ERROR: (ErrorCode, &str) = (
    ErrorCode::RequestCanceled,
    "language server is shutting down",
);

pub(super) fn error_response(id: RequestId, (code, msg): (ErrorCode, &str)) -> Response {
    Response::new_err(id, code as i32, msg.into())
}

pub(super) fn merge_code_actions(
    req: &PendingRequest,
    resp: &mut Result<SourceReply, SourceFailure>,
) {
    let Some(narrowing) = req.narrowing.as_ref() else {
        return;
    };
    let Ok(SourceReply::CodeActions(semantic)) = resp else {
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
        SourceFailure::InvalidName(err) => {
            let msg = err.to_string();
            crate::protocol::with_data(
                Response::new_err(id, ErrorCode::InvalidParams as i32, msg.clone()),
                serde_json::json!({ "kind": err.kind(), "message": msg }),
            )
        }
        SourceFailure::Failed(err) => {
            tracing::error!(error = ?err, "a source request failed");
            error_response(id, REQUEST_FAILED_ERROR)
        }
    }
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn source_reply(&mut self, id: RequestId, reply: SourceReply) -> Result<()> {
        let mut reply = self.shaped_reply(reply);
        self.workspace().features.reply(&mut reply);
        self.reply(Response::new_ok(id, reply))
    }

    fn shaped_reply(&mut self, mut reply: SourceReply) -> SourceReply {
        if let SourceReply::CodeLenses(Some(lenses)) = &mut reply {
            for lens in lenses {
                let Some(cmd) = &lens.command else {
                    continue;
                };
                if let Some(route) = protocol::preview_route(cmd) {
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
                if let lsp_types::CodeActionOrCommand::CodeAction(action) = action
                    && let Some(edit) = &mut action.edit
                {
                    self.version_document_edits(edit);
                }
            }
        }
        reply
    }

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
        changes.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
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

    pub(super) fn reject(&mut self, id: RequestId, code: ErrorCode, msg: &str) -> Result<()> {
        self.reply(error_response(id, (code, msg)))
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
            let resp = failed_response(7.into(), SourceFailure::of(failure));
            let err = resp.response_result.unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidParams as i32);
            assert_eq!(err.data.unwrap()["kind"], "conflictingSourceText");
            assert!(err.message.contains("save or close"));
        }
    }

    #[test]
    fn entity_edits_keep_open_versions() {
        use crate::connection::tests::{open_document, ready_at, ready_workspace};
        let dir = tempfile::tempdir().unwrap();
        let mut conn = ready_at(Vec::new(), dir.path().to_owned());
        let caps = serde_json::from_value(serde_json::json!({
            "workspace": {"workspaceEdit": {"documentChanges": true}}
        }))
        .unwrap();
        ready_workspace(&mut conn).features = crate::capabilities::ClientFeatures::new(&caps);
        let def = crate::uri::from_file_path(&dir.path().join("definitions.typ")).unwrap();
        let ref_uri = crate::uri::from_file_path(&dir.path().join("author.typ")).unwrap();
        open_document(&mut conn, &def, 7, "#define(\"sku:α\")");
        open_document(&mut conn, &ref_uri, 3, "#link(\"sku:α\")");
        let edits = [def.clone(), ref_uri.clone()]
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
        conn.source_reply(
            19.into(),
            SourceReply::Rename(Some(lsp_types::WorkspaceEdit {
                changes: Some(edits),
                ..Default::default()
            })),
        )
        .unwrap();
        let result = response(&mut conn).response_result.unwrap();
        assert!(result.get("changes").is_none());
        let changes = result["documentChanges"].as_array().unwrap();
        assert_eq!(changes.len(), 2);
        for (uri, ver) in [(def, 7), (ref_uri, 3)] {
            let change = changes
                .iter()
                .find(|c| c["textDocument"]["uri"] == uri.as_str())
                .unwrap();
            assert_eq!(change["textDocument"]["version"], ver);
            assert_eq!(change["edits"][0]["newText"], "\"sku:β\"");
        }
    }

    #[test]
    fn preview_lens_carries_url() {
        let mut conn = ready(Vec::new());
        conn.default_origin = Some("http://localhost:4321".to_owned());
        let uri: Uri = "file:///site/page.typ".parse().unwrap();
        let lens = |route: Option<&str>| {
            let mut args = vec![serde_json::Value::String(uri.as_str().to_owned())];
            if let Some(route) = route {
                args.push(serde_json::Value::String(route.to_owned()));
            }
            lsp_types::CodeLens {
                range: lsp_types::Range::default(),
                command: Some(lsp_types::Command {
                    title: "Save and preview".to_owned(),
                    command: protocol::PREVIEW_COMMAND.to_owned(),
                    arguments: Some(args),
                }),
                data: None,
            }
        };
        conn.source_reply(
            1.into(),
            SourceReply::CodeLenses(Some(vec![lens(Some("/page/")), lens(None)])),
        )
        .unwrap();
        let lenses: Vec<lsp_types::CodeLens> =
            serde_json::from_value(response(&mut conn).response_result.unwrap()).unwrap();
        assert_eq!(
            lenses[0].data,
            Some(serde_json::json!({ "route": "/page/", "url": "http://localhost:4321/page/" }))
        );
        assert_eq!(lenses[1].data, None);
    }
}
