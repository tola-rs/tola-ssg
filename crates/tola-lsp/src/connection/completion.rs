//! What a finished lane job leaves behind: the check's published diagnostics and announcements, the
//! replies its completion maps to, and the refresh a pull client is asked for.

use std::io::Write;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::MessageType;
use lsp_types::notification::{self, Notification as LspNotification};
use lsp_types::request::{self, Request as LspRequest};
use tola_build::diagnostic::Diagnostic;

use crate::compiler::{
    CheckedSources, QueryRequest, SourceCompilation, SourceFailure, SourceJob, checked_root,
};
use crate::diagnostic::refine_unread_configuration;
use crate::protocol::CheckProgress;

use super::replies::{CANCELLED_ERROR, error_response, failed_response, merge_code_actions};
use super::{Connection, Phase};

/// What the author hears, in the client's own message channel, when no world resolved at all.
const UNREAD_CONFIGURATION_ANNOUNCEMENT: &str =
    "Tola could not read this site's configuration; fix the problem reported in `tola.toml`";

/// What the author hears when a check's own work failed rather than the site.
const PANICKED_CHECK_ANNOUNCEMENT: &str =
    "Tola could not check this site; restart the language server";

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(crate) fn completed(&mut self, compilation: SourceCompilation) -> Result<()> {
        match compilation {
            SourceCompilation::Checked {
                checked_revision,
                checked,
            } => {
                if self.running_check == Some(checked_revision) {
                    self.running_check = None;
                }
                // A check that finished is the one the author was waiting for, so the next check is
                // one of their own edits rather than the site's first.
                self.warm = true;
                if checked_revision == self.revision
                    && !self.checking.token().is_cancelled()
                    && !matches!(checked, Err(SourceFailure::Cancelled))
                {
                    if let Err(SourceFailure::Failed(error)) = &checked
                        && error
                            .chain()
                            .any(|cause| cause.is::<crate::server::PanickedJob>())
                    {
                        // The check's own lane failed, so no outcome exists for this revision: the
                        // author is told the check failed rather than that the site does not
                        // compile, and nothing is published as this revision's result.
                        self.report_source_check(CheckProgress::NotChecked)?;
                        self.announce_panic()?;
                    } else {
                        self.announced_panic = false;
                        let compiled = checked.as_ref().is_ok_and(|checked| checked.compiled);
                        self.checked = Some((checked_revision, compiled));
                        let state = if compiled {
                            CheckProgress::Checked
                        } else {
                            CheckProgress::Failed
                        };
                        if let Ok(checked) = checked {
                            if let Some(paths) = checked.read_paths.as_ref() {
                                self.read_paths = paths.iter().cloned().collect();
                            }
                            self.announce(&checked)?;
                            self.publish(checked, checked_revision)?;
                            // The check built this revision's index itself, so a correction waiting
                            // for one reads it now instead of paying for a second walk.
                            if self.selection_build == Some(checked_revision) {
                                self.selection_build = None;
                            }
                            self.queries.retain(|job| {
                                !matches!(job, SourceJob::Selection(job) if job.revision == checked_revision)
                            });
                            self.complete_waiting_code_actions(checked_revision)?;
                            self.refresh_diagnostics()?;
                        }
                        self.report_source_check(state)?;
                    }
                }
                self.finish_progress(checked_revision)?;
            }
            SourceCompilation::Answered {
                id,
                serial,
                response,
            } => {
                // Client IDs may be reused while an older cancelled job finishes.
                if self
                    .requests
                    .get(&id)
                    .is_some_and(|request| request.serial == serial)
                {
                    let request = self.requests.remove(&id).expect("matching source request");
                    let Phase::Ready(_) = &self.phase else {
                        return Ok(());
                    };
                    let mut response = response;
                    merge_code_actions(&request, &mut response);
                    // Every client-facing source reply is shaped in the one order — this
                    // connection's own shaping, then the capability projection — whether or not
                    // the request waited for the checked world.
                    if request.canceller.token().is_cancelled() {
                        response = Err(SourceFailure::Cancelled);
                    }
                    match response {
                        Ok(reply) => self.source_reply(id, reply)?,
                        Err(SourceFailure::Cancelled) => {
                            self.reply(error_response(id, CANCELLED_ERROR))?
                        }
                        Err(SourceFailure::Failed(error)) => {
                            self.reply(failed_response(id, error))?
                        }
                    }
                }
            }
            SourceCompilation::Continued(continued) => {
                // The request keeps its id, serial, cancellation, and view: the compiler lane
                // answers it exactly once, while a superseded or cancelled analysis enqueues
                // nothing and replies nothing.
                if self
                    .requests
                    .get(&continued.id)
                    .is_some_and(|request| request.serial == continued.serial)
                {
                    let Phase::Ready(workspace) = &self.phase else {
                        return Ok(());
                    };
                    self.queries.push_back(SourceJob::Query(QueryRequest {
                        id: continued.id,
                        serial: continued.serial,
                        sources: self.source_inputs(continued.root, continued.cancellation),
                        view: continued.view,
                        package_sources: workspace.package_sources.clone(),
                        routes_as_hints: workspace.routes_as_hints,
                        check_progress: self.check_progress(),
                        query: continued.query,
                    }));
                }
            }
            SourceCompilation::Selected {
                revision,
                root,
                selected,
            } => {
                if self.selection_build == Some(revision) {
                    self.selection_build = None;
                }
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
                    Err(SourceFailure::Failed(_)) => {
                        self.fail_waiting_code_actions(revision)?;
                    }
                }
            }
            // A release answers nothing: the next read of a revision re-derives what it needs.
            SourceCompilation::Released => {}
        }
        Ok(())
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
        workspace.diagnostics.publish(
            &mut self.writer,
            root,
            &entry,
            &self.open,
            &workspace.features,
            &checked.selected,
            diagnostics,
        )
    }

    /// Ask a client that pulls to read the diagnostics the check just produced.
    ///
    /// The request is global, so one refresh makes the client re-read every document it pulls.
    fn refresh_diagnostics(&mut self) -> Result<()> {
        let refresh = match &self.phase {
            Phase::Ready(workspace) => {
                workspace.features.pull_diagnostics && workspace.features.diagnostic_refresh
            }
            _ => false,
        };
        if refresh {
            self.send_request(request::WorkspaceDiagnosticRefresh::METHOD, ())?;
        }
        Ok(())
    }

    /// Tell the author, once per state, about the one failure nothing else reports.
    ///
    /// A check that could not read the site's own configuration publishes its diagnostics on that
    /// document, which the author may never open; every other failure reaches them as a diagnostic
    /// on a document they can see. So this failure is announced once, when it starts, and the
    /// recoverable ones stay in the editor's own reporting.
    fn announce(&mut self, checked: &CheckedSources) -> Result<()> {
        let failed = checked.configuration.is_none();
        if failed == self.announced_configuration_failure {
            return Ok(());
        }
        self.announced_configuration_failure = failed;
        if !failed {
            return Ok(());
        }
        self.show_message(
            MessageType::WARNING,
            UNREAD_CONFIGURATION_ANNOUNCEMENT.to_owned(),
        )
    }

    /// Show the author one message the client puts in front of them.
    fn show_message(&mut self, typ: MessageType, message: String) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::{
        dynamic_import_position, messages, open_document, ready, ready_at, references_request,
        response, uninitialized,
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
        connection
            .completed(crate::compiler::analyze(
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                superseded,
            ))
            .unwrap();
        connection
            .completed(crate::compiler::analyze(
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                current,
            ))
            .unwrap();
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

    /// A continuation can no longer answer its request once the client cancelled that request, so
    /// it enqueues nothing and replies nothing.
    #[test]
    fn stale_continuations_enqueue_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let mut connection = ready_at(Vec::new(), directory.path().to_path_buf());
        let (uri, position) = dynamic_import_position(&mut connection, directory.path());
        connection
            .request(references_request(&uri, position))
            .unwrap();
        let pending = connection
            .next_analysis()
            .expect("source analysis admission");
        let SourceCompilation::Continued(continued) = crate::compiler::analyze(
            &mut crate::analysis::DiskSources::default(),
            &mut crate::analysis::GraphCache::default(),
            pending,
        ) else {
            panic!("a dynamic import's references defer");
        };
        let _ = connection
            .notification(Notification::new(
                notification::Cancel::METHOD.into(),
                json!({"id":8}),
            ))
            .unwrap();
        assert_eq!(
            response(&mut connection).response_result.unwrap_err().code,
            CANCELLED_ERROR.0 as i32
        );
        // The client reuses the id: the finished analysis no longer owns the pending request.
        connection
            .request(references_request(&uri, position))
            .unwrap();
        let current = connection.next_analysis().expect("replacement admission");
        assert_ne!(current.serial, continued.serial);
        connection
            .completed(SourceCompilation::Continued(continued))
            .unwrap();
        assert!(
            connection.writer.is_empty(),
            "a stale continuation replies nothing"
        );
        assert!(connection.next_job(Instant::now()).is_none());
    }

    /// A check that resolved no world says what that costs the author, and the reason Typst wrote
    /// stays below it: the author acts on the sentence Tola writes, not on Typst's.
    #[test]
    fn check_without_world_says_what_it_costs() {
        let mut connection = ready(Vec::new());
        let revision = connection.revision;
        connection
            .publish(
                CheckedSources {
                    configuration: None,
                    compiled: false,
                    diagnostics: vec![Diagnostic::at_path(
                        crate::codes::editor::CONFIGURATION,
                        tola_build::diagnostic::Severity::Error,
                        "tola.toml",
                        "`--pure` cannot use `--package-path`",
                    )],
                    read_paths: None,
                    selected: Arc::new(SelectedInterfaces::default()),
                },
                revision,
            )
            .unwrap();
        let published: lsp_types::PublishDiagnosticsParams = messages(&mut connection)
            .into_iter()
            .find_map(|message| match message {
                Message::Notification(notification)
                    if notification.method == "textDocument/publishDiagnostics" =>
                {
                    serde_json::from_value(notification.params).ok()
                }
                _ => None,
            })
            .expect("a published report");
        assert_eq!(published.diagnostics.len(), 1);
        let message = &published.diagnostics[0].message;
        assert_eq!(
            message,
            "Tola could not read the site's configuration\n\
             note: `--pure` cannot use `--package-path`\n\
             note: pages, labels, and hovers stay unavailable until it loads\n\
             help: fix the problem reported in `tola.toml`",
            "{message}"
        );
    }

    /// A site whose configuration could not be read is announced to the author once, and announced
    /// again after a configuration that read again breaks once more.
    #[test]
    fn configuration_failure_is_announced_once() {
        let mut connection = ready(Vec::new());
        fn failed_check(connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) {
            let revision = connection.revision;
            connection
                .completed(SourceCompilation::Checked {
                    checked_revision: revision,
                    checked: Ok(CheckedSources {
                        configuration: None,
                        compiled: false,
                        diagnostics: Vec::new(),
                        read_paths: None,
                        selected: Arc::new(SelectedInterfaces::default()),
                    }),
                })
                .unwrap();
        }
        fn shown(connection: &mut Connection<Vec<u8>, impl Fn(&[Diagnostic])>) -> Vec<String> {
            messages(connection)
                .into_iter()
                .filter_map(|message| match message {
                    Message::Notification(notification)
                        if notification.method == notification::ShowMessage::METHOD =>
                    {
                        let params: lsp_types::ShowMessageParams =
                            serde_json::from_value(notification.params).unwrap();
                        Some(params.message)
                    }
                    _ => None,
                })
                .collect()
        }
        failed_check(&mut connection);
        failed_check(&mut connection);
        let announced = shown(&mut connection);
        assert_eq!(announced.len(), 1, "{announced:?}");
        assert!(
            announced[0].contains("could not read this site's configuration"),
            "{announced:?}"
        );
        assert!(announced[0].contains("tola.toml"), "{announced:?}");
        // A configuration that reads again is not a failure, so the next one is announced afresh.
        connection.announced_configuration_failure = false;
        failed_check(&mut connection);
        assert_eq!(shown(&mut connection).len(), 1);
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
        let config = Arc::new(
            tola_build::config::loading::load_site_config(
                Some(&directory.path().join("tola.toml")),
                tola_typst::PackageLocations::default(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        );
        let mut compiler = crate::compiler::SourceCompiler::new(
            move |_, _| Ok(crate::server::ServedWorkspace::Site(Arc::clone(&config))),
            tola_build::BuildResources::default(),
        );
        let mut connection = uninitialized();
        connection.request(Request::new(1.into(), request::Initialize::METHOD.into(), json!({
            "rootUri":crate::uri::from_file_path(directory.path()).unwrap(),
            "capabilities":{"textDocument":{"diagnostic":{}},"workspace":{"diagnostics":{"refreshSupport":true}}}
        }))).unwrap();
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
    }
}
