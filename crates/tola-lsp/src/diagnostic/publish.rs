//! Sending one check's diagnostics: every URI a document's report reaches, and what it has.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lsp_server::{Message, Notification};
use lsp_types::notification::{Notification as LspNotification, PublishDiagnostics};
use lsp_types::{Diagnostic as EditorDiagnostic, PublishDiagnosticsParams, Uri};
use tola_build::diagnostic::Diagnostic;
use tola_typst::typst::syntax::VirtualRoot;
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
        let unread = unread_import_diagnostics(unread_documents(open, None).0, selected);
        for diagnostic in diagnostics.into_iter().chain(unread) {
            let (path, diagnostic) = editor_diagnostic(
                diagnostic,
                root,
                entry,
                &self.root,
                fold_related,
                &mut texts,
            )?;
            documents.entry(path).or_default().push(diagnostic);
        }
        let mut current = BTreeSet::new();
        for (path, diagnostics) in documents {
            for (uri, version) in self.recipients(&path, open)? {
                current.insert(uri.as_str().to_owned());
                self.report(output, features, uri, version, diagnostics.clone())?;
            }
        }
        for path in self.held_documents(open) {
            for (uri, version) in self.recipients(&path, open)? {
                if current.insert(uri.as_str().to_owned()) {
                    self.report(output, features, uri, version, Vec::new())?;
                }
            }
        }
        let stale: Vec<String> = self.published.difference(&current).cloned().collect();
        for uri in stale {
            let version = open.version(&uri);
            let uri: Uri = uri.parse().context("published document URI is invalid")?;
            self.report(output, features, uri, version, Vec::new())?;
        }
        // A report answers only for a document this check addressed: any other document pulls as
        // an empty report, which is what this publish just told the client about it.
        self.reports.retain(|uri, _| current.contains(uri));
        self.published = current;
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
        publish_document(output, &parameters)
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
    /// The file id has the site-relative path, and the root the id was built against is the
    /// workspace root this projection already keys its URIs by, so joining the two names the
    /// document exactly as the open-source table does. A document of another root — a builtin
    /// package's own file — is not this site's to report on.
    fn held_documents(&self, open: &OpenSources) -> Vec<PathBuf> {
        open.view()
            .iter()
            .filter(|snapshot| matches!(snapshot.source.id().root(), VirtualRoot::Project))
            .map(|snapshot| {
                self.root
                    .resolved()
                    .join(snapshot.source.id().vpath().get_without_slash())
            })
            .collect()
    }
}

fn publish_document(output: &mut impl Write, parameters: &PublishDiagnosticsParams) -> Result<()> {
    let message: Message = Notification::new(PublishDiagnostics::METHOD.into(), parameters).into();
    message.write(output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::tests::{features, open_source, reports, selected_interfaces};
    use lsp_server::Message;
    use lsp_types::notification::PublishDiagnostics;
    use lsp_types::{
        DiagnosticSeverity, NumberOrString, Position, PublishDiagnosticsParams, Range,
    };
    use tola_build::diagnostic::{Diagnostic, Location, Severity, SourcePosition, SourceRange};

    fn published(input: &mut std::io::Cursor<Vec<u8>>) -> PublishDiagnosticsParams {
        let Message::Notification(message) = Message::read(input).unwrap().unwrap() else {
            panic!("expected diagnostic notification");
        };
        message.extract(PublishDiagnostics::METHOD).unwrap()
    }

    #[test]
    fn cleared_diagnostics_have_open_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.typ");
        let uri = crate::uri::from_file_path(&path).unwrap();
        let mut open = OpenSources::new(directory.path());
        open_source(&mut open, uri.clone(), 1);
        let mut client = ClientDiagnostics::new(directory.path().to_path_buf());
        let mut bytes = Vec::new();
        let selected = selected_interfaces(directory.path(), &open);
        client
            .publish(
                &mut bytes,
                directory.path(),
                &path,
                &open,
                &features(),
                &selected,
                vec![Diagnostic::at_path(
                    tola_build::codes::typst::COMPILE,
                    Severity::Error,
                    path.to_string_lossy(),
                    "source error",
                )],
            )
            .unwrap();
        open.apply(Notification::new(
            lsp_types::notification::DidChangeTextDocument::METHOD.into(),
            lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier::new(uri.clone(), 3),
                content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "Corrected source".into(),
                }],
            },
        ))
        .unwrap();
        let selected = selected_interfaces(directory.path(), &open);
        client
            .publish(
                &mut bytes,
                directory.path(),
                &path,
                &open,
                &features(),
                &selected,
                Vec::new(),
            )
            .unwrap();
        let mut input = std::io::Cursor::new(bytes);
        let first = published(&mut input);
        assert_eq!(first.uri, uri);
        assert_eq!(first.version, Some(1));
        assert_eq!(first.diagnostics[0].message, "source error");
        assert_eq!(
            published(&mut input),
            PublishDiagnosticsParams::new(uri, Vec::new(), Some(3))
        );
        assert!(Message::read(&mut input).unwrap().is_none());
    }

    #[test]
    fn package_diagnostics_reach_each_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.typ");
        let uri = crate::uri::from_file_path(&path).unwrap();
        let alias: Uri = uri
            .as_str()
            .replace("document.typ", "%64ocument.typ")
            .parse()
            .unwrap();
        let mut open = OpenSources::new(directory.path());
        for (uri, version) in [(uri.clone(), 7), (alias.clone(), 9)] {
            open_source(&mut open, uri, version);
        }
        let location = Location {
            path: "document.typ".into(),
            line: Some(3),
            column: Some(1),
            range: Some(SourceRange {
                start: SourcePosition {
                    line: 2,
                    character: 4096,
                },
                end: SourcePosition {
                    line: 3,
                    character: 8,
                },
            }),
            source_lines: Vec::new(),
        };
        let mut diagnostic = Diagnostic::at_path(
            tola_build::codes::typst::COMPILE,
            Severity::Error,
            "@preview/package:1.0.0/lib.typ",
            "primary failure",
        )
        .with_note("retained note")
        .with_help("retained help");
        diagnostic.help.push(tola_build::diagnostic::Help {
            message: "located help".into(),
            location: Some(location.clone()),
        });
        diagnostic.trace.push(tola_build::diagnostic::TraceFrame {
            message: "calling source".into(),
            location: Some(location),
        });
        diagnostic.trace.push(tola_build::diagnostic::TraceFrame {
            message: "unlocated trace".into(),
            location: None,
        });
        let mut client = ClientDiagnostics::new(directory.path().to_path_buf());
        let mut bytes = Vec::new();
        let selected = selected_interfaces(directory.path(), &open);
        client
            .publish(
                &mut bytes,
                directory.path(),
                &directory.path().join("index.typ"),
                &open,
                &features(),
                &selected,
                vec![diagnostic],
            )
            .unwrap();
        let mut input = std::io::Cursor::new(bytes);
        let mut versions = BTreeMap::new();
        for parameters in reports(&mut input) {
            versions.insert(parameters.uri.as_str().to_owned(), parameters.version);
            let [diagnostic] = parameters.diagnostics.as_slice() else {
                panic!("expected one complete diagnostic");
            };
            assert_eq!(
                diagnostic.range,
                Range::new(Position::new(2, 4096), Position::new(3, 8))
            );
            assert_eq!(
                diagnostic.code,
                Some(NumberOrString::String("typst.compile".into()))
            );
            assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
            // The message reaches the editor without the package location the failure was raised
            // in: a package's own file is not the author's to open.
            assert!(
                diagnostic.message.contains("primary failure"),
                "{diagnostic:?}"
            );
            assert!(
                !diagnostic
                    .message
                    .contains("@preview/package:1.0.0/lib.typ"),
                "{diagnostic:?}"
            );
            // Every note, help, and trace reaches the editor exactly once: text that has nowhere
            // to point stays in the message, and text that does point travels with its related
            // location instead.
            for text in ["retained note", "retained help", "unlocated trace"] {
                assert_eq!(
                    diagnostic.message.matches(text).count(),
                    1,
                    "missing or repeated diagnostic detail: {text}"
                );
            }
            for text in ["located help", "calling source"] {
                assert!(
                    !diagnostic.message.contains(text),
                    "located text repeated in the message: {text}"
                );
            }
            let related = diagnostic.related_information.as_ref().unwrap();
            assert_eq!(
                related
                    .iter()
                    .map(|related| related.message.as_str())
                    .collect::<Vec<_>>(),
                ["located help", "calling source"]
            );
            assert!(
                related.iter().all(|related| related.location.uri == uri
                    && related.location.range == diagnostic.range)
            );
        }
        assert_eq!(
            versions,
            BTreeMap::from([
                (uri.as_str().to_owned(), Some(7)),
                (alias.as_str().to_owned(), Some(9))
            ])
        );
    }

    /// A document the client never opened has no URI of its own, so its report is addressed under
    /// the root the client named: the resolved spelling would name a document no editor opened.
    #[test]
    fn unopened_document_follows_the_client_root_spelling() {
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
                    tola_build::codes::typst::COMPILE,
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
