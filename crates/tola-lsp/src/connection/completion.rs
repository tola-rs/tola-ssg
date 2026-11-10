//! What a finished lane job leaves behind: the check's published diagnostics and announcements, the
//! replies its completion maps to, and the refresh a pull client is asked for.

use std::io::Write;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::MessageType;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use tola_build::config::ResolvedSiteConfig;
use tola_diagnostic::Diagnostic;

use crate::compiler::{
    CheckedSources, QueryRequest, SourceCompilation, SourceFailure, SourceJob, checked_root,
};
use crate::diagnostic::{PublishInputs, refine_unread_configuration};
use crate::protocol::{CheckProgress, SourceReply};

use super::replies::{REVISION_CHANGED_ERROR, error_response, failed_response, merge_code_actions};
use super::{Connection, PendingRequest, Phase};

/// What the author hears when a check's own work failed rather than the site.
const PANICKED_CHECK_ANNOUNCEMENT: &str =
    "Tola could not check this site; restart the language server";

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(crate) fn completed(&mut self, compilation: SourceCompilation) -> Result<()> {
        match compilation {
            SourceCompilation::Checked {
                checked_revision,
                checked,
            } => self.accept_check(checked_revision, checked)?,
            SourceCompilation::Answered {
                id,
                serial,
                configuration,
                read_paths,
                response,
            } => {
                // Client IDs may be reused while an older cancelled job finishes.
                if self.accepts_reply(&id, serial)? {
                    let request = self.requests.remove(&id).expect("matching source request");
                    let Phase::Ready(_) = &self.phase else {
                        return Ok(());
                    };
                    let mut response = honour_cancellation(&request, response);
                    merge_code_actions(&request, &mut response);
                    // A rejected query still depends on the files that made it fail.
                    if !matches!(&response, Err(SourceFailure::Cancelled)) {
                        self.query_read_paths
                            .extend(read_paths.into_iter().flat_map(|path| {
                                let physical = tola_input::normalize_existing_prefix(&path);
                                [path, physical]
                            }));
                        self.watch_sources()?;
                    }
                    match response {
                        Ok(reply) => self.accept_answer(id, configuration, reply)?,
                        Err(failure) => self.reply_failure(id, failure)?,
                    }
                }
            }
            SourceCompilation::Tokens {
                id,
                serial,
                read,
                response,
            } => {
                if self.accepts_reply(&id, serial)? {
                    let request = self.requests.remove(&id).expect("matching tokens request");
                    match honour_cancellation(&request, response) {
                        Ok(tokens) => self.commit_tokens(id, read, tokens)?,
                        Err(failure) => self.reply_failure(id, failure)?,
                    }
                }
            }
            SourceCompilation::Continued(continued) => {
                // The request keeps its id, serial, cancellation, and view: the compiler lane
                // answers it exactly once, while a superseded or cancelled analysis enqueues
                // nothing and replies nothing.
                if self.accepts_reply(&continued.id, continued.serial)? {
                    let Phase::Ready(workspace) = &self.phase else {
                        return Ok(());
                    };
                    self.queries.push_back(SourceJob::Query(QueryRequest {
                        id: continued.id,
                        serial: continued.serial,
                        sources: continued.sources,
                        view: continued.view,
                        routes_as_hints: workspace.routes_as_hints,
                        check_progress: self.check_progress(),
                        query: continued.query,
                    }));
                }
            }
            SourceCompilation::Selected {
                revision,
                serial,
                root,
                selected,
            } => {
                if !self
                    .selection_build
                    .as_ref()
                    .is_some_and(|build| build.serial == serial)
                {
                    return Ok(());
                }
                self.cancel_selection();
                match selected {
                    // A build a source change cancelled describes a revision the author has left:
                    // its waiting requests were superseded with it, and no later one may read its
                    // index.
                    Ok(selected) if revision == self.revision => {
                        let Phase::Ready(workspace) = &mut self.phase else {
                            return Ok(());
                        };
                        workspace
                            .diagnostics
                            .keep_selection(revision, &root, selected);
                        self.complete_waiting_code_actions(revision)?;
                    }
                    Ok(_) | Err(SourceFailure::Cancelled) => {}
                    Err(
                        SourceFailure::Failed(_)
                        | SourceFailure::Panicked
                        | SourceFailure::InvalidName(_),
                    ) => {
                        self.fail_waiting_code_actions(revision)?;
                    }
                }
            }
            // A release answers nothing: the next read of a revision re-derives what it needs.
            SourceCompilation::Released => {}
        }
        Ok(())
    }

    /// Take one finished site check: the lane's own failure, or the outcome this revision answers
    /// from.
    ///
    /// A run a newer revision superseded, a cancelled one, and a panicked one all publish nothing
    /// as this revision's result, and every one of them ends its progress report here.
    fn accept_check(
        &mut self,
        checked_revision: u64,
        checked: Result<CheckedSources, SourceFailure>,
    ) -> Result<()> {
        if self.running_check == Some(checked_revision) {
            self.running_check = None;
        }
        // A check that finished is the one the author was waiting for, so the next check is one of
        // their own edits rather than the site's first.
        self.warm = true;
        if checked_revision == self.revision
            && !self.checking.token().is_cancelled()
            && !matches!(checked, Err(SourceFailure::Cancelled))
        {
            match checked {
                Err(SourceFailure::Panicked) => {
                    // The check's own lane failed, so no outcome exists for this revision: the
                    // author is told the check failed rather than that the site does not compile.
                    self.report_source_check(CheckProgress::NotChecked)?;
                    self.announce_panic()?;
                }
                checked => self.apply_check(checked_revision, checked)?,
            }
        }
        self.finish_progress(checked_revision)
    }

    /// Record one site check's outcome, answer from it, and report the progress it established.
    fn apply_check(
        &mut self,
        checked_revision: u64,
        checked: Result<CheckedSources, SourceFailure>,
    ) -> Result<()> {
        self.announced_panic = false;
        let Ok(checked) = checked else {
            self.checked = Some(super::CompletedCheck {
                revision: checked_revision,
                usable_world: false,
            });
            return self.report_source_check(CheckProgress::Failed);
        };
        let compiled = checked.compiled;
        self.checked = Some(super::CompletedCheck {
            revision: checked_revision,
            usable_world: compiled,
        });
        let progress = if compiled {
            CheckProgress::Checked
        } else {
            CheckProgress::Failed
        };
        let configuration = checked.configuration.clone();
        if let Some(paths) = checked.read_paths.as_ref() {
            self.read_paths = paths.iter().cloned().collect();
        }
        self.publish(checked, checked_revision)?;
        self.watch_sources()?;
        self.refresh_semantic_tokens(checked_revision)?;
        // The check built this revision's index itself, so a correction waiting for one reads it
        // now instead of paying for a second walk.
        if self
            .selection_build
            .as_ref()
            .is_some_and(|build| build.revision == checked_revision)
        {
            self.cancel_selection();
        }
        self.complete_waiting_code_actions(checked_revision)?;
        self.refresh_diagnostics()?;
        match configuration {
            Some(configuration) => self.report_prepared_site(progress, &configuration),
            None => self.report_source_check(progress),
        }
    }

    /// Answer one source failure in the protocol's own shape.
    fn reply_failure(&mut self, id: lsp_server::RequestId, failure: SourceFailure) -> Result<()> {
        self.reply(failed_response(id, failure))
    }

    /// Answer one query from the reply its job produced.
    ///
    /// A configuration this connection has not accepted yet is reported first, so the client reads
    /// the package roots before a location inside them.
    fn accept_answer(
        &mut self,
        id: lsp_server::RequestId,
        configuration: Option<Arc<ResolvedSiteConfig>>,
        reply: SourceReply,
    ) -> Result<()> {
        if let Some(configuration) = configuration
            && !self
                .configuration
                .as_ref()
                .is_some_and(|accepted| Arc::ptr_eq(accepted, &configuration))
        {
            self.report_prepared_site(self.check_progress(), &configuration)?;
        }
        self.source_reply(id, reply)
    }

    fn accepts_reply(&mut self, id: &lsp_server::RequestId, serial: u64) -> Result<bool> {
        let Some(request) = self
            .requests
            .get(id)
            .filter(|request| request.serial == serial)
        else {
            return Ok(false);
        };
        let current = request
            .document_revision
            .as_ref()
            .is_none_or(|(uri, revision)| {
                self.open
                    .view()
                    .get(uri)
                    .is_some_and(|parsed| parsed.revision == *revision)
            });
        if current {
            return Ok(true);
        }
        self.requests
            .remove(id)
            .expect("matching source request")
            .canceller
            .cancel();
        self.discard_cancelled_jobs();
        self.reply(error_response(id.clone(), REVISION_CHANGED_ERROR))?;
        Ok(false)
    }

    fn publish(&mut self, checked: CheckedSources, checked_revision: u64) -> Result<()> {
        let Phase::Ready(workspace) = &mut self.phase else {
            return Ok(());
        };
        (self.record_diagnostics)(&checked.diagnostics);
        let mut diagnostics = checked.diagnostics;
        // A check that resolved no world has nothing to say about any page, so what the author
        // reads is what that costs them, and the reason Typst wrote stays below it.
        if checked.configuration.is_none() {
            refine_unread_configuration(&mut diagnostics);
        }
        if let Some(config) = &checked.configuration {
            self.configuration = Some(Arc::clone(config));
            self.open.rebase(config.get_root());
            // A file-change notification refuses what this configuration's own writing produces;
            // Tola's own state directory and the build lock are the connection's own set, applied
            // whether or not a check has resolved a configuration. A vendor candidate is refused
            // here too: it is written by a preparation, not by the author.
            let mut generated = tola_typst::SourceBoundary::new(config.get_root(), false);
            for exclusion in config.generated_exclusions() {
                generated = generated.excluding(exclusion.directory);
            }
            self.open.exclude_generated(generated);
        }
        let root = checked_root(checked.configuration.as_ref(), &workspace.root);
        let entry = checked.configuration.as_ref().map_or_else(
            || workspace.root.join("site.typ"),
            |config| config.build().entry.clone(),
        );
        // The index answers for the revision this check checked: the report below reads it now,
        // and a correction requested while that revision is still the checked one reads it too.
        workspace
            .diagnostics
            .keep_selection(checked_revision, root, Arc::clone(&checked.selected));
        workspace.diagnostics.publish(PublishInputs {
            output: &mut self.writer,
            root,
            entry: &entry,
            open: &self.open,
            features: &workspace.features,
            selected: &checked.selected,
            diagnostics,
        })
    }

    /// Ask a client that pulls to read the diagnostics the check just produced.
    ///
    /// The request is global, so one refresh makes the client re-read every document it pulls.
    fn refresh_diagnostics(&mut self) -> Result<()> {
        let Phase::Ready(workspace) = &self.phase else {
            return Ok(());
        };
        if workspace.features.pull_diagnostics && workspace.features.diagnostic_refresh {
            self.send_request(request::WorkspaceDiagnosticRefresh::METHOD, ())?;
        }
        Ok(())
    }

    /// Show the author one message the client puts in front of them.
    pub(super) fn show_message(&mut self, typ: MessageType, message: String) -> Result<()> {
        self.notify(
            notification::ShowMessage::METHOD,
            lsp_types::ShowMessageParams { typ, message },
        )
    }

    /// Tell the author about a check whose own work failed, once per run of failures: the flag
    /// stands until a check completes again.
    fn announce_panic(&mut self) -> Result<()> {
        if self.announced_panic {
            return Ok(());
        }
        self.announced_panic = true;
        self.show_message(MessageType::ERROR, PANICKED_CHECK_ANNOUNCEMENT.to_owned())
    }
}

/// The answer a finished job leaves for its client: a request that client cancelled answers as
/// cancelled, whatever the job produced.
fn honour_cancellation<T>(
    request: &PendingRequest,
    response: Result<T, SourceFailure>,
) -> Result<T, SourceFailure> {
    if request.canceller.token().is_cancelled() {
        Err(SourceFailure::Cancelled)
    } else {
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        answer_analysis, messages, open_document, ready, ready_at, ready_workspace, response,
        uninitialized,
    };
    use lsp_server::{ErrorCode, Message, Notification, Request};
    use lsp_types::notification::{self, Notification as LspNotification};
    use lsp_types::request::{self, Request as LspRequest};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;
    use tola_typst::typst::syntax::Source;
    use tola_typst_syntax::names::SelectedInterfaces;

    #[test]
    fn prepared_site_status_precedes_definition() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let cache = directory.path().join("cache");
        std::fs::create_dir(&site).unwrap();
        std::fs::create_dir(&cache).unwrap();
        std::fs::create_dir(site.join("assets")).unwrap();
        std::fs::write(site.join("icon.svg"), "<svg/>").unwrap();
        let configuration = site.join("tola.toml");
        std::fs::write(
            &configuration,
            "[assets]\nfiles = [{ source = \"icon.svg\", url = \"/icon.svg\" }]\ntrees = [{ source = \"assets\", url-prefix = \"/assets\" }]\n",
        )
        .unwrap();
        let locations =
            tola_typst::PackageLocations::from_absolute_roots(None, Some(cache.clone())).unwrap();
        let configuration = Arc::new(
            tola_build::config::loading::load_site_config(
                Some(&configuration),
                locations,
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        );
        let expected_entry_uri = crate::uri::ClientRoot::new(&site)
            .address(&configuration.build().entry)
            .unwrap();
        let config_uri = crate::uri::from_file_path(&site.join("tola.toml")).unwrap();
        let mut connection = ready_at(Vec::new(), site);
        ready_workspace(&mut connection).source_check_status = true;
        let opened = connection
            .notification(Notification::new(
                notification::DidOpenTextDocument::METHOD.into(),
                lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem::new(
                        config_uri.clone(),
                        "toml".into(),
                        7,
                        std::fs::read_to_string(configuration.config_path()).unwrap(),
                    ),
                },
            ))
            .unwrap();
        assert!(opened.is_continue());
        messages(&mut connection);
        let id = 7.into();
        let serial = connection.admit(&id).unwrap().unwrap().serial;
        connection
            .completed(SourceCompilation::Answered {
                id,
                serial,
                configuration: Some(configuration),
                read_paths: Vec::new(),
                response: Ok(SourceReply::Definition(Some(
                    lsp_types::GotoDefinitionResponse::Scalar(lsp_types::Location {
                        uri: crate::uri::from_file_path(&cache.join("local/demo/0.1.0/lib.typ"))
                            .unwrap(),
                        range: lsp_types::Range::default(),
                    }),
                ))),
            })
            .unwrap();

        let sent = messages(&mut connection);
        let [Message::Notification(status), Message::Response(reply)] = sent.as_slice() else {
            panic!("package roots must precede the definition reply: {sent:?}");
        };
        assert_eq!(status.method, "tola/sourceCheckStatus");
        let resolved = tola_input::normalize_existing_prefix(&cache);
        assert_eq!(
            status.params["site"]["packageRoots"],
            serde_json::json!([crate::uri::from_file_path(&resolved).unwrap().as_str()])
        );
        assert_eq!(status.params["site"]["entry"], expected_entry_uri.as_str());
        assert_eq!(
            status.params["site"]["configVersions"][config_uri.as_str()],
            7
        );
        let site_root = tola_input::normalize_existing_prefix(&directory.path().join("site"));
        assert_eq!(
            status.params["site"]["root"],
            crate::uri::from_file_path(&site_root).unwrap().as_str()
        );
        assert_eq!(
            status.params["site"]["assetSources"],
            serde_json::json!({
                "files": [crate::uri::from_file_path(&site_root.join("icon.svg")).unwrap().as_str()],
                "trees": [crate::uri::from_file_path(&site_root.join("assets")).unwrap().as_str()],
            })
        );
        let generated = status.params["site"]["generatedRoots"].as_array().unwrap();
        assert!(generated.contains(&serde_json::json!(
            crate::uri::from_file_path(&site_root.join("public"))
                .unwrap()
                .as_str()
        )));
        assert!(reply.response_result.is_ok());
        connection
            .report_source_check(CheckProgress::Checking)
            .unwrap();
        let sent = messages(&mut connection);
        let [Message::Notification(transient)] = sent.as_slice() else {
            panic!("a transient status notification");
        };
        assert!(transient.params.get("site").is_none());
    }

    #[test]
    fn configuration_errors_stay_in_diagnostics() {
        let mut connection = ready(Vec::new());
        connection
            .completed(SourceCompilation::Checked {
                checked_revision: connection.revision,
                checked: Ok(CheckedSources {
                    configuration: None,
                    compiled: false,
                    diagnostics: vec![Diagnostic::at_path(
                        tola_diagnostic::codes::config::TOML,
                        tola_diagnostic::Severity::Error,
                        "tola.toml",
                        "unclosed table",
                    )],
                    read_paths: None,
                    selected: Arc::new(SelectedInterfaces::default()),
                }),
            })
            .unwrap();
        let sent = messages(&mut connection);
        assert!(sent.iter().any(|message| matches!(message,
            Message::Notification(message)
                if message.method == notification::PublishDiagnostics::METHOD
                && !message.params["diagnostics"].as_array().unwrap().is_empty()
        )));
        assert!(!sent.iter().any(|message| matches!(message,
            Message::Notification(message) if message.method == notification::ShowMessage::METHOD
        )));
    }

    #[test]
    fn reused_id_rejects_stale_analysis() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let uri = crate::uri::from_file_path(&directory.path().join("page.typ")).unwrap();
        let opened = "#let count = 1\n#count";
        // The superseded and the current analysis have to report different ranges, so the edit
        // renames the binding instead of replacing its value.
        let edited = "#let counter = 2\n#counter";
        open_document(&mut connection, &uri, 1, opened);
        let opened_source = Source::detached(opened);
        let byte = opened.rfind("count").unwrap();
        let position = crate::position::utf16_range(opened_source.lines(), byte..byte)
            .unwrap()
            .start;
        let references = || {
            Request::new(
                8.into(),
                request::References::METHOD.into(),
                json!({
                    "textDocument":{"uri":uri.as_str()}, "position":position, "context":{"includeDeclaration":true}
                }),
            )
        };
        connection.request(references()).unwrap();
        let superseded = connection
            .next_analysis()
            .expect("source analysis admission");
        let _ = connection.notification(Notification::new(
            notification::DidChangeTextDocument::METHOD.into(),
            json!({"textDocument":{"uri":uri.as_str(),"version":2},"contentChanges":[{"text":edited}]}),
        )).unwrap();
        assert_eq!(
            response(&mut connection).response_result.unwrap_err().code,
            ErrorCode::ContentModified as i32
        );
        connection.request(references()).unwrap();
        let current = connection
            .next_analysis()
            .expect("replacement source analysis");
        answer_analysis(&mut connection, superseded);
        answer_analysis(&mut connection, current);
        let locations: Vec<lsp_types::Location> =
            serde_json::from_value(response(&mut connection).response_result.unwrap()).unwrap();
        let edited_source = Source::detached(edited);
        let expected: Vec<_> = edited
            .match_indices("counter")
            .map(|(start, name)| {
                lsp_types::Location::new(
                    uri.clone(),
                    crate::position::utf16_range(edited_source.lines(), start..start + name.len())
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(locations, expected);
    }

    #[test]
    fn pull_warning_refreshes_current_report() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("tola.toml"), "").unwrap();
        std::fs::write(
            directory.path().join("site.typ"),
            "#document(\"index.html\", format:\"html\")[]",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("content/page.typ"),
            "#metadata((title:\"Legacy\")) <tola-meta>",
        )
        .unwrap();
        let config = crate::compiler::tests::site_configuration(directory.path(), "");
        let mut compiler = crate::compiler::SourceCompiler::new(
            move |_, _| Ok(crate::server::ServedWorkspace::Site(Arc::clone(&config))),
            tola_build::BuildResources::default(),
        );
        let mut connection = uninitialized();
        connection.request(Request::new(1.into(), request::Initialize::METHOD.into(), json!({
            "workspaceFolders":[{"uri":crate::uri::from_file_path(directory.path()).unwrap(),"name":"site"}],
            "capabilities":{"textDocument":{"diagnostic":{}},"workspace":{"diagnostics":{"refreshSupport":true}}}
        }))).unwrap();
        let initialized = response(&mut connection).response_result.unwrap();
        assert_eq!(
            initialized["capabilities"]["diagnosticProvider"]["interFileDependencies"],
            true
        );
        assert_eq!(
            initialized["capabilities"]["diagnosticProvider"]["workspaceDiagnostics"],
            true
        );
        let _ = connection
            .notification(Notification::new(
                notification::Initialized::METHOD.into(),
                json!({}),
            ))
            .unwrap();
        messages(&mut connection);
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        let written = messages(&mut connection);
        assert!(
            written
                .iter()
                .any(|message| matches!(message, Message::Request(request)
            if request.method == "workspace/diagnostic/refresh"))
        );
        assert!(!written.iter().any(
            |message| matches!(message, Message::Notification(notification)
            if notification.method == notification::PublishDiagnostics::METHOD)
        ));
        connection.request(Request::new(2.into(), request::DocumentDiagnosticRequest::METHOD.into(), json!({
            "textDocument":{"uri":crate::uri::from_file_path(&directory.path().join("site.typ")).unwrap()}
        }))).unwrap();
        let report = response(&mut connection).response_result.unwrap();
        assert!(
            report["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |diagnostic| diagnostic["code"] == "source.declaration_deprecated"
                        && diagnostic["severity"] == 2
                )
        );

        let uri = crate::uri::from_file_path(&directory.path().join("site.typ")).unwrap();
        let pull = |connection: &mut Connection<Vec<u8>, _>, id: i32, previous| {
            connection
                .request(Request::new(
                    id.into(),
                    request::WorkspaceDiagnosticRequest::METHOD.into(),
                    json!({"previousResultIds": previous}),
                ))
                .unwrap();
            response(connection).response_result.unwrap()["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|report| report["uri"] == uri.as_str())
                .unwrap()
                .clone()
        };
        let full = pull(&mut connection, 3, json!([]));
        assert_eq!(full["items"], report["items"]);
        let previous = json!([{"uri": uri, "value": full["resultId"]}]);
        assert_eq!(
            pull(&mut connection, 4, previous.clone())["kind"],
            "unchanged"
        );

        let page = directory.path().join("content/page.typ");
        std::fs::remove_file(&page).unwrap();
        let _ = connection
            .notification(Notification::new(
                notification::DidChangeWatchedFiles::METHOD.into(),
                json!({"changes": [{"uri": crate::uri::from_file_path(&page).unwrap(), "type": 3}]}),
            ))
            .unwrap();
        let checked = compiler.compile(connection.next_job(Instant::now()).unwrap());
        connection.completed(checked).unwrap();
        assert_eq!(pull(&mut connection, 5, previous)["items"], json!([]));
    }
}
