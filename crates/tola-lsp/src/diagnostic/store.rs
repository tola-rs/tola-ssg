//! The reports one connection holds: each document's last report, the result id it answers a
//! pull with, and the selection index a completed check built.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{
    Diagnostic as EditorDiagnostic, DocumentDiagnosticReport, FullDocumentDiagnosticReport,
    NumberOrString, RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
    UnchangedDocumentDiagnosticReport, Uri,
};
use tola_typst_syntax::names;

pub(crate) struct ClientDiagnostics {
    /// The workspace root in both spellings: the path Tola resolves and the one the editor named.
    ///
    /// A document the client holds is addressed by the URI the client opened it with; every other
    /// document — one the author never opened — is addressed under this root.
    pub(super) root: crate::uri::ClientRoot,
    pub(super) published: BTreeSet<String>,
    /// Each document's last report, keyed by the URI the client addressed it by.
    ///
    /// A report outlives the check that produced it: a pull request answers with it. Owned by this
    /// connection, replaced when a check reports the document again.
    pub(super) reports: BTreeMap<String, Report>,
    /// Where result ids come from: a report whose items change takes the next one.
    pub(super) result_serial: u64,
    /// What every site source selects from every other, as of the revision one check read.
    ///
    /// The check lane builds this while it checks and the connection keeps what the check returned;
    /// a later request reads it only for the exact revision and site it names, so an index the
    /// author has already left behind never answers about documents it does not describe.
    pub(super) selection: Option<(u64, PathBuf, Arc<names::SelectedInterfaces>)>,
}

impl ClientDiagnostics {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self::with_root(crate::uri::ClientRoot::new(&root))
    }

    pub(super) fn with_root(root: crate::uri::ClientRoot) -> Self {
        Self {
            root,
            published: BTreeSet::new(),
            reports: BTreeMap::new(),
            result_serial: 0,
            selection: None,
        }
    }

    /// Keep the selection index one completed check built for the revision it checked.
    ///
    /// The root is resolved here as it was when the index was built, so the editor's spelling of a
    /// symlinked site and the configuration's resolved one key the same index.
    pub(crate) fn keep_selection(
        &mut self,
        revision: u64,
        root: &Path,
        selected: Arc<names::SelectedInterfaces>,
    ) {
        self.selection = Some((
            revision,
            tola_build::filesystem::normalize_existing_prefix(root),
            selected,
        ));
    }

    /// The selection index a completed check returned for `revision` under `root`.
    ///
    /// Nothing here builds one: a revision no completed check covered, or a site a different check
    /// covered, proves nothing about the documents a request reads, and the file-scope statements
    /// only an index can license stay withheld rather than judged from a stale one.
    pub(super) fn selection_for(
        &self,
        revision: u64,
        root: &Path,
    ) -> Option<Arc<names::SelectedInterfaces>> {
        let root = tola_build::filesystem::normalize_existing_prefix(root);
        self.selection
            .as_ref()
            .filter(|(cached, cached_root, _)| *cached == revision && *cached_root == root)
            .map(|(_, _, selected)| Arc::clone(selected))
    }

    /// The report one document's pull request answers with, for the version the client has synced.
    ///
    /// A client that echoes a report's own result id back is told `Unchanged`, which is what keeps
    /// a slow client from re-rendering items a newer revision did not change. A document no check
    /// has reported yet answers with no items rather than with an error: the site may not
    /// have been checked since the client opened it.
    pub(crate) fn pull(&self, uri: &Uri, previous: Option<&str>, synced: Option<i32>) -> Pulled {
        let report = self.reports.get(uri.as_str());
        // A report that names no version, or another version, cannot be vouched for.
        if let (Some(report), Some(synced)) = (report, synced)
            && report.version != Some(synced)
        {
            return Pulled::Stale;
        }
        let result_id =
            report.map_or_else(|| self.unreported_id(), |report| report.result_id.clone());
        if previous == Some(result_id.as_str()) {
            return Pulled::Report(DocumentDiagnosticReport::Unchanged(
                RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                },
            ));
        }
        Pulled::Report(DocumentDiagnosticReport::Full(
            RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items: report.map_or_else(Vec::new, |report| report.diagnostics.clone()),
                },
            },
        ))
    }

    /// The result id a document no check has reported answers with.
    ///
    /// No report has taken a serial yet, so the first one to arrive changes this id and the client
    /// pulls again instead of trusting an empty report.
    fn unreported_id(&self) -> String {
        format!("tola/{}", self.result_serial)
    }

    /// The diagnostics a correction reads for one document, as the server reported them.
    ///
    /// A client that reads no `data` cannot echo a cause back, so a connection serving one
    /// substitutes these for the diagnostics that client sent with its code-action request: they
    /// are the items the check reported, with `data` still in place. Unread-import items are
    /// absent, because `unused_imports` recomputes those from the text the editor holds. Answers
    /// nothing for a document no check reported, or one whose report is not for `synced`.
    pub(crate) fn cause_diagnostics(
        &self,
        uri: &Uri,
        synced: Option<i32>,
    ) -> Option<&[EditorDiagnostic]> {
        let report = self.reports.get(uri.as_str())?;
        if let Some(synced) = synced
            && report.version != Some(synced)
        {
            return None;
        }
        Some(&report.cause_diagnostics)
    }

    /// Keep one document's report, giving it a new result id only when its items change.
    ///
    /// The id is what the client echoes back to ask whether a report still stands, so it changes
    /// with the items and with nothing else: not with the check's revision, and not with the
    /// version the client reported the document at. Two items are equal only when every field the
    /// client sees is equal, the cause payload included, which is what lets the id stand in for a
    /// digest of them.
    pub(super) fn keep(
        &mut self,
        uri: &Uri,
        version: Option<i32>,
        diagnostics: Vec<EditorDiagnostic>,
        cause_diagnostics: Vec<EditorDiagnostic>,
    ) {
        let result_id = match self.reports.get(uri.as_str()) {
            Some(report) if report.diagnostics == diagnostics => report.result_id.clone(),
            _ => {
                self.result_serial += 1;
                format!("tola/{}", self.result_serial)
            }
        };
        self.reports.insert(
            uri.as_str().to_owned(),
            Report {
                version,
                diagnostics,
                cause_diagnostics,
                result_id,
            },
        );
    }
}

/// What one document's pull request answers with.
///
/// A report is only answered for the version the client has synced: one that names another
/// version, or none at all, is refused, which is the protocol's shape for a report the server
/// cannot vouch for, and the client asks again once the site's own state settles.
#[derive(Debug, PartialEq)]
pub(crate) enum Pulled {
    /// The report to display, or the word that the one the client holds still stands.
    Report(DocumentDiagnosticReport),
    /// The report Tola holds is not one for the version the client has synced.
    Stale,
}

/// One document's report: what the last check said about it, and the id it answers a pull with.
pub(super) struct Report {
    /// The version the client reported the document at when this report was sent.
    version: Option<i32>,
    diagnostics: Vec<EditorDiagnostic>,
    /// The items whose `data` has a cause a correction reads, as the server built them: a
    /// client that reads no `data` never echoes one back, so the server answers from here.
    cause_diagnostics: Vec<EditorDiagnostic>,
    /// Identifies these items; a client echoes it back on its next pull for the document.
    result_id: String,
}

/// Whether one item has a cause a correction reads.
///
/// An unread import is left out: `unused_imports` recomputes that correction from the text the
/// editor holds, so a client's echo never has to hold it.
pub(super) fn carries_cause(diagnostic: &EditorDiagnostic) -> bool {
    let unread_import = match &diagnostic.code {
        Some(NumberOrString::String(code)) => {
            code.as_str() == tola_build::codes::check::UNUSED_IMPORT.as_str()
        }
        _ => false,
    };
    diagnostic.data.is_some() && !unread_import
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::tests::{Site, features, open_source, selected_interfaces};
    use crate::sources::OpenSources;
    use lsp_server::Notification;
    use lsp_types::notification::Notification as LspNotification;
    use tola_build::diagnostic::{Diagnostic, DiagnosticCause, Severity};

    /// One open document and the checks a test reports on it.
    struct Checked {
        root: tempfile::TempDir,
        uri: Uri,
        open: OpenSources,
        client: ClientDiagnostics,
        bytes: Vec<u8>,
    }

    impl Checked {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let uri = crate::uri::from_file_path(&root.path().join("document.typ")).unwrap();
            let mut open = OpenSources::new(root.path());
            open_source(&mut open, uri.clone(), 1);
            Self {
                client: ClientDiagnostics::new(root.path().to_path_buf()),
                root,
                uri,
                open,
                bytes: Vec::new(),
            }
        }

        fn path(&self) -> PathBuf {
            self.root.path().join("document.typ")
        }

        fn check(&mut self, messages: &[&str]) {
            let path = self.path();
            let selected = selected_interfaces(self.root.path(), &self.open);
            self.client
                .publish(
                    &mut self.bytes,
                    self.root.path(),
                    &path,
                    &self.open,
                    &features(),
                    &selected,
                    messages
                        .iter()
                        .map(|message| {
                            Diagnostic::at_path(
                                tola_build::codes::typst::COMPILE,
                                Severity::Error,
                                path.to_string_lossy(),
                                *message,
                            )
                        })
                        .collect(),
                )
                .unwrap();
        }

        fn pull(&self, previous: Option<&str>) -> Pulled {
            self.client
                .pull(&self.uri, previous, self.open.version(self.uri.as_str()))
        }

        fn edited(&mut self) {
            self.open
                .apply(Notification::new(
                    lsp_types::notification::DidChangeTextDocument::METHOD.into(),
                    lsp_types::DidChangeTextDocumentParams {
                        text_document: lsp_types::VersionedTextDocumentIdentifier::new(
                            self.uri.clone(),
                            2,
                        ),
                        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                            range: None,
                            range_length: None,
                            text: "corrected".into(),
                        }],
                    },
                ))
                .unwrap();
        }
    }

    /// The result id and the item messages one full report names.
    fn reported(pulled: Pulled) -> (String, Vec<String>) {
        let Pulled::Report(DocumentDiagnosticReport::Full(full)) = pulled else {
            panic!("expected a full report");
        };
        (
            full.full_document_diagnostic_report
                .result_id
                .expect("a full report names its items"),
            full.full_document_diagnostic_report
                .items
                .iter()
                .map(|item| item.message.clone())
                .collect(),
        )
    }

    /// A client that declares no diagnostic capabilities at all.
    fn bare_features() -> crate::capabilities::ClientFeatures {
        crate::capabilities::ClientFeatures::new(&lsp_types::ClientCapabilities::default())
    }

    #[test]
    fn repeated_items_keep_their_result_id() {
        let mut checked = Checked::new();
        checked.check(&["source error"]);
        let (id, items) = reported(checked.pull(None));
        assert_eq!(items, ["source error"]);
        checked.check(&["source error"]);
        assert_eq!(
            checked.pull(Some(&id)),
            Pulled::Report(DocumentDiagnosticReport::Unchanged(
                RelatedUnchangedDocumentDiagnosticReport {
                    related_documents: None,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id: id,
                    },
                }
            ))
        );
    }

    #[test]
    fn pull_after_the_version_moves_is_stale() {
        let mut checked = Checked::new();
        checked.check(&["source error"]);
        let (id, _) = reported(checked.pull(None));
        checked.edited();
        assert_eq!(checked.pull(Some(&id)), Pulled::Stale);
    }

    /// A report sent while the document was not open cannot answer for the version the client
    /// opened it at.
    #[test]
    fn open_document_refuses_report_of_unknown_version() {
        let mut site = Site::new(&[("content/page.typ", "Body\n")]);
        site.publish(vec![Diagnostic::at_path(
            tola_build::codes::typst::COMPILE,
            Severity::Error,
            "content/page.typ",
            "source error",
        )]);
        let uri = crate::uri::from_file_path(&site.physical.join("content/page.typ")).unwrap();
        assert!(matches!(
            site.client.pull(&uri, None, None),
            Pulled::Report(_)
        ));
        site.open("content/page.typ", "Edited body\n");
        assert_eq!(
            site.client
                .pull(&uri, None, site.open.version(uri.as_str())),
            Pulled::Stale
        );
    }

    #[test]
    fn changed_items_take_fresh_result_id() {
        let mut checked = Checked::new();
        checked.check(&["first"]);
        let (first, _) = reported(checked.pull(None));
        checked.check(&["first", "second"]);
        let (second, items) = reported(checked.pull(Some(&first)));
        assert_ne!(first, second);
        assert_eq!(items, ["first", "second"]);
    }

    #[test]
    fn cleared_documents_pull_no_items() {
        let mut checked = Checked::new();
        checked.check(&["source error"]);
        let (reported_id, _) = reported(checked.pull(None));
        checked.check(&[]);
        let (cleared, items) = reported(checked.pull(Some(&reported_id)));
        assert_ne!(reported_id, cleared);
        assert!(items.is_empty());
    }

    #[test]
    fn unreported_document_pulls_empty_report() {
        let checked = Checked::new();
        let (id, items) = reported(checked.pull(None));
        assert!(!id.is_empty());
        assert!(items.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn selected_interfaces_follow_the_resolved_root() {
        let mut site = Site::new(&[
            ("content/exported.typ", "#let value = 1\n"),
            (
                "content/consumer.typ",
                "#import \"exported.typ\": value\n#value\n",
            ),
        ]);
        site.open("content/reader.typ", "#let value = 1\n");
        let alias = site.root.path().join("alias");
        std::os::unix::fs::symlink(&site.physical, &alias).unwrap();

        let exported =
            crate::identity::path_id(&site.physical.join("content/exported.typ"), &site.physical)
                .expect("a site source");
        // The editor's spelling reaches the same sources as the resolved root, so the index built
        // for it selects exactly what the consumer's import selects.
        let aliased = selected_interfaces(&alias, &site.open);
        assert!(
            !aliased.reads_nothing_from(exported, "value"),
            "the consumer's import selects the name under the editor's spelling"
        );
        // A check builds one index per site, and the connection keys it by the resolved root: the
        // editor's spelling and the resolved one name that same index.
        let mut client = ClientDiagnostics::new(alias.clone());
        client.keep_selection(1, &alias, aliased);
        let named = client
            .selection_for(1, &site.physical)
            .expect("the site's own index");
        assert!(
            !named.reads_nothing_from(exported, "value"),
            "the resolved root answers with the index built under the editor's spelling"
        );
    }

    /// A client that reads no diagnostic data is answered from the causes the server kept.
    #[test]
    fn causes_survive_without_data_support() {
        let mut site = Site::new(&[("content/page.typ", "Body\n")]);
        let uri = crate::uri::from_file_path(&site.physical.join("content/page.typ")).unwrap();
        let cause = DiagnosticCause::UnknownVariable {
            name: "missing".into(),
        };
        site.publish_with(
            &bare_features(),
            vec![
                Diagnostic::at_path(
                    tola_build::codes::typst::COMPILE,
                    Severity::Error,
                    "content/page.typ",
                    "unknown variable `missing`",
                )
                .with_cause(cause.clone()),
            ],
        );
        let diagnostics = site.diagnostics();
        let [published] = diagnostics.as_slice() else {
            panic!("one diagnostic: {diagnostics:?}");
        };
        assert_eq!(published.data, None);
        let kept = site
            .client
            .cause_diagnostics(&uri, site.open.version(uri.as_str()))
            .expect("the report the check just published");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].data, Some(serde_json::to_value(&cause).unwrap()));
    }
}
