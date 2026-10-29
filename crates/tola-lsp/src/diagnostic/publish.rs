//! Sending one check's diagnostics: every URI a document's report reaches, and what it has.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lsp_server::{Message, Notification};
use lsp_types::notification::{Notification as LspNotification, PublishDiagnostics};
use lsp_types::{Diagnostic as EditorDiagnostic, PublishDiagnosticsParams, Uri};
use tola_diagnostic::{Diagnostic, Severity};
use tola_typst_syntax::names;

use crate::sources::OpenSources;

use super::projection::{DiagnosticTexts, editor_diagnostic};
use super::store::{ClientDiagnostics, carries_cause};
use super::unread::{unread_documents, unread_import_diagnostics};

impl ClientDiagnostics {
    /// Report one completed check's diagnostics to the client.
    ///
    /// A check reports every document it has something to say about, *including* the documents it
    /// found nothing wrong with: a client that waits for the first report before it calls itself
    /// ready otherwise waits forever on a site that compiles. A document no client holds is
    /// addressed by its own path, so an editor reading the site from disk still learns what a
    /// check found there.
    #[expect(
        clippy::too_many_arguments,
        reason = "where the report goes, which site and document it addresses, the revision's own documents, its selection index, and its diagnostics are independent inputs"
    )]
    pub(crate) fn publish(
        &mut self,
        output: &mut impl Write,
        root: &Path,
        entry: &Path,
        open: &OpenSources,
        features: &crate::capabilities::ClientFeatures,
        selected: &names::SelectedInterfaces,
        diagnostics: Vec<Diagnostic>,
    ) -> Result<()> {
        let mut documents = BTreeMap::<PathBuf, Vec<EditorDiagnostic>>::new();
        // A client that reads no related information still reads the sentences one has, so a
        // located help or trace folds into the message it belongs to below.
        let fold_related = !features.reads_related_information();
        let mut texts = DiagnosticTexts::new(open);
        let unread = unread_import_diagnostics(unread_documents(open, None).documents, selected);
        for diagnostic in diagnostics.into_iter().chain(unread) {
            // A diagnostic no site document reaches is logged instead, and the log line is built
            // before the projection consumes the diagnostic.
            let log_line = diagnostic
                .producer
                .is_some()
                .then(|| log_message(&diagnostic));
            let projected = editor_diagnostic(
                diagnostic,
                root,
                entry,
                &self.root,
                fold_related,
                &mut texts,
            )?;
            if let Some((path, diagnostic)) = projected {
                documents.entry(path).or_default().push(diagnostic);
            } else if let Some((message, typ)) = log_line {
                let notification: Message = Notification::new(
                    <lsp_types::notification::LogMessage as LspNotification>::METHOD.into(),
                    lsp_types::LogMessageParams { typ, message },
                )
                .into();
                notification.write(output)?;
            }
        }
        let mut current = BTreeSet::new();
        for (path, diagnostics) in documents {
            for (uri, version) in self.recipients(&path, open)? {
                current.insert(uri.as_str().to_owned());
                self.report(output, features, uri, version, diagnostics.clone())?;
            }
        }
        for path in self.held_documents(root, open) {
            for (uri, version) in self.recipients(&path, open)? {
                if current.insert(uri.as_str().to_owned()) {
                    self.report(output, features, uri, version, Vec::new())?;
                }
            }
        }
        let stale: Vec<_> = self
            .reports
            .keys()
            .filter(|uri| !current.contains(*uri))
            .cloned()
            .collect();
        for uri in stale {
            let version = open.version(&uri);
            let uri: Uri = uri.parse().context("published document URI is invalid")?;
            self.report(output, features, uri, version, Vec::new())?;
        }
        // A report answers only for a document this check addressed: any other document pulls as
        // an empty report, which is what this publish just told the client about it.
        self.reports.retain(|uri, _| current.contains(uri));
        Ok(())
    }

    /// Send one document's report and keep it for the client's next pull.
    fn report(
        &mut self,
        output: &mut impl Write,
        features: &crate::capabilities::ClientFeatures,
        uri: Uri,
        version: Option<i32>,
        diagnostics: Vec<EditorDiagnostic>,
    ) -> Result<()> {
        // The causes a correction reads, kept as the server built them: a client that reads no
        // `data` never echoes one back, so a connection serving one answers from here instead.
        let cause_diagnostics: Vec<EditorDiagnostic> = diagnostics
            .iter()
            .filter(|diagnostic| carries_cause(diagnostic))
            .cloned()
            .collect();
        let mut parameters = PublishDiagnosticsParams::new(uri.clone(), diagnostics, version);
        features.diagnostics(&mut parameters);
        // The version is the server's own record of what it reported, not a wire field: a client
        // that never asked for versions still has its reports compared against the text it synced.
        self.keep(
            &uri,
            version,
            parameters.diagnostics.clone(),
            cause_diagnostics,
        );
        // A client that pulls asks for what it shows, and its own collection is separate from the
        // pushed one, so pushing these items too would show them twice.
        if features.pull_diagnostics {
            return Ok(());
        }
        let message: Message =
            Notification::new(PublishDiagnostics::METHOD.into(), &parameters).into();
        message.write(output)?;
        Ok(())
    }

    /// Every URI and version the client holds for one path, or the path's own URI when it holds
    /// none.
    fn recipients(&self, path: &Path, open: &OpenSources) -> Result<Vec<(Uri, Option<i32>)>> {
        let held: Vec<(Uri, Option<i32>)> = open
            .versions_for_path(path)
            .map(|(uri, version)| {
                Ok((
                    uri.parse().context("open document URI is invalid")?,
                    Some(version),
                ))
            })
            .collect::<Result<_>>()?;
        if held.is_empty() {
            return Ok(vec![(self.root.address(path)?, None)]);
        }
        Ok(held)
    }

    /// The site's own files the client holds open.
    ///
    /// The file id has the site-relative path, and the root the id was built against is the site
    /// root this check resolved, so joining the two names the document exactly as the open-source
    /// table does. A document of another root — a builtin package's own file — is not this site's
    /// to report on.
    fn held_documents(&self, root: &Path, open: &OpenSources) -> Vec<PathBuf> {
        let root = tola_build::filesystem::normalize_existing_prefix(root);
        open.view()
            .iter()
            .map(|parsed| root.join(parsed.source.id().vpath().get_without_slash()))
            .collect()
    }
}

/// The log line one diagnostic no site document reaches reads as.
fn log_message(diagnostic: &Diagnostic) -> (String, lsp_types::MessageType) {
    let (level, typ) = match diagnostic.severity {
        Severity::Error => ("error", lsp_types::MessageType::ERROR),
        Severity::Warning => ("warning", lsp_types::MessageType::WARNING),
        Severity::Info => ("info", lsp_types::MessageType::INFO),
        Severity::Hint => ("hint", lsp_types::MessageType::LOG),
    };
    let mut message = format!(
        "{level}[{}]: {}",
        diagnostic.code_label(),
        diagnostic.message
    );
    for help in diagnostic.distinct_help() {
        message.push_str(&format!("\nhelp: {}", help.message));
    }
    for related in &diagnostic.related {
        let location = related
            .location
            .as_ref()
            .map(|location| {
                let mut path = location.path.clone();
                if let Some(line) = location.line {
                    path.push_str(&format!(":{line}"));
                }
                path.push_str(": ");
                path
            })
            .unwrap_or_default();
        message.push_str(&format!("\nrelated: {location}{}", related.message));
    }
    (message, typ)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::tests::{features, reports, selected_interfaces};
    use tola_diagnostic::{Diagnostic, Severity};

    #[test]
    fn vanished_reports_clear_once() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let open = OpenSources::new(&root);
        let selected = selected_interfaces(&root, &open);
        let mut client = ClientDiagnostics::new(root.clone());
        let mut sent = Vec::new();
        for path in [Some("first.typ"), Some("second.typ"), None, None] {
            let mut bytes = Vec::new();
            client
                .publish(
                    &mut bytes,
                    &root,
                    &root.join("site.typ"),
                    &open,
                    &features(),
                    &selected,
                    path.into_iter()
                        .map(|path| {
                            Diagnostic::at_path(
                                tola_diagnostic::codes::typst::COMPILE,
                                Severity::Error,
                                path,
                                "source error",
                            )
                        })
                        .collect(),
                )
                .unwrap();
            sent.push(
                reports(&mut std::io::Cursor::new(bytes))
                    .into_iter()
                    .map(|report| {
                        (
                            crate::uri::to_file_path(report.uri.as_str())
                                .unwrap()
                                .file_name()
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned(),
                            report.diagnostics.is_empty(),
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            sent,
            [
                vec![("first.typ".into(), false)],
                vec![("second.typ".into(), false), ("first.typ".into(), true)],
                vec![("second.typ".into(), true)],
                Vec::new(),
            ]
        );
    }
    /// A document the client never opened has no URI of its own, so its report is addressed under
    /// the root the client named: the resolved spelling would name a document no editor opened.
    #[test]
    fn unopened_document_uses_client_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let resolved = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("client-site");
        let page = resolved.join("content/page.typ");
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "Body").unwrap();
        // The client named the site under its own spelling; the check spells the file as it
        // resolved it.
        let mut client = ClientDiagnostics::with_root(crate::uri::ClientRoot::with_resolved(
            &spelled, &resolved,
        ));
        let mut bytes = Vec::new();
        let open = OpenSources::new(&resolved);
        let selected = selected_interfaces(&resolved, &open);
        client
            .publish(
                &mut bytes,
                &resolved,
                &resolved.join("index.typ"),
                &open,
                &features(),
                &selected,
                vec![Diagnostic::at_path(
                    tola_diagnostic::codes::typst::COMPILE,
                    Severity::Error,
                    page.to_string_lossy(),
                    "source error",
                )],
            )
            .unwrap();
        let mut input = std::io::Cursor::new(bytes);
        let addressed: Vec<String> = reports(&mut input)
            .into_iter()
            .map(|parameters| parameters.uri.as_str().to_owned())
            .collect();
        assert_eq!(
            addressed,
            [
                crate::uri::from_file_path(&spelled.join("content/page.typ"))
                    .unwrap()
                    .as_str()
                    .to_owned()
            ]
        );
    }
}
