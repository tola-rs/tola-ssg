//! Projection of complete compiler diagnostics into one connection's document URIs.
//!
//! A check's diagnostics are projected per document and kept as the reports the client pulls, the
//! causes a correction reads, and the unread imports still to remove.

mod projection;
mod publish;
mod refine;
mod store;
mod unread;

pub(crate) use refine::refine_unread_configuration;
pub(crate) use store::{ClientDiagnostics, Pulled};
pub(crate) use unread::UnreadImports;

#[cfg(test)]
pub(crate) mod tests {
    //! Builders the diagnostic module tree's tests drive: a temporary site with open documents,
    //! and the reports its client was sent.

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use lsp_server::{Message, Notification};
    use lsp_types::notification::{
        DidOpenTextDocument, Notification as LspNotification, PublishDiagnostics,
    };
    use lsp_types::{
        Diagnostic as EditorDiagnostic, DidOpenTextDocumentParams, PublishDiagnosticsParams,
        TextDocumentItem, Uri,
    };
    use tola_build::diagnostic::Diagnostic;
    use tola_typst_syntax::names;

    use crate::sources::OpenSources;

    use super::store::ClientDiagnostics;

    pub(crate) fn open_source(open: &mut OpenSources, uri: Uri, version: i32) {
        open.apply(Notification::new(
            DidOpenTextDocument::METHOD.into(),
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(uri, "typst".into(), version, "source".into()),
            },
        ))
        .unwrap();
    }

    /// Every report one client was sent, as the notification has it.
    pub(crate) fn reports(input: &mut std::io::Cursor<Vec<u8>>) -> Vec<PublishDiagnosticsParams> {
        let mut sent = Vec::new();
        while let Some(message) = Message::read(input).unwrap() {
            let Message::Notification(message) = message else {
                panic!("expected diagnostic notification");
            };
            sent.push(message.extract(PublishDiagnostics::METHOD).unwrap());
        }
        sent
    }

    pub(crate) fn features() -> crate::capabilities::ClientFeatures {
        crate::capabilities::ClientFeatures::new(&serde_json::from_value(serde_json::json!({
            "textDocument": { "publishDiagnostics": { "versionSupport": true, "relatedInformation": true } }
        })).unwrap())
    }

    /// The selection index a check of these open documents builds, which the check lane hands the
    /// connection in `CheckedSources` and every report of that revision reads.
    pub(crate) fn selected_interfaces(
        root: &Path,
        open: &OpenSources,
    ) -> Arc<names::SelectedInterfaces> {
        crate::analysis::site_selected_interfaces(
            root,
            &open.view(),
            &mut crate::analysis::DiskSources::default(),
            &tola_build::cancellation::BuildCancellation::new(),
        )
        .expect("the site's sources are indexed")
    }

    /// One site on disk with the documents a test opens, and the reports its client was sent.
    pub(crate) struct Site {
        pub(crate) root: tempfile::TempDir,
        /// The root as the server spells it: the filesystem's own spelling, symlinks followed.
        pub(crate) physical: PathBuf,
        pub(crate) open: OpenSources,
        pub(crate) client: ClientDiagnostics,
        bytes: Vec<u8>,
    }

    impl Site {
        pub(crate) fn new(files: &[(&str, &str)]) -> Self {
            let root = tempfile::tempdir().unwrap();
            for (path, text) in files {
                let path = root.path().join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            let physical = tola_build::filesystem::normalize_existing_prefix(root.path());
            let open = OpenSources::new(&physical);
            Self {
                client: ClientDiagnostics::new(physical.clone()),
                root,
                physical,
                open,
                bytes: Vec::new(),
            }
        }

        pub(crate) fn open(&mut self, path: &str, text: &str) {
            let uri = crate::uri::from_file_path(&self.physical.join(path)).unwrap();
            self.open
                .apply(Notification::new(
                    lsp_types::notification::DidOpenTextDocument::METHOD.into(),
                    lsp_types::DidOpenTextDocumentParams {
                        text_document: lsp_types::TextDocumentItem::new(
                            uri,
                            "typst".into(),
                            1,
                            text.into(),
                        ),
                    },
                ))
                .unwrap();
        }

        pub(crate) fn changed(&mut self, paths: &[&str]) {
            let changes: Vec<lsp_types::FileEvent> = paths
                .iter()
                .map(|path| lsp_types::FileEvent {
                    uri: crate::uri::from_file_path(&self.physical.join(path)).unwrap(),
                    typ: lsp_types::FileChangeType::CHANGED,
                })
                .collect();
            self.open
                .apply(Notification::new(
                    lsp_types::notification::DidChangeWatchedFiles::METHOD.into(),
                    lsp_types::DidChangeWatchedFilesParams { changes },
                ))
                .unwrap();
        }

        pub(crate) fn check(&mut self) {
            self.publish(Vec::new());
        }

        pub(crate) fn publish(&mut self, diagnostics: Vec<Diagnostic>) {
            self.publish_with(&tagged_features(), diagnostics);
        }

        /// One check's diagnostics as a client with the given capabilities reads them.
        pub(crate) fn publish_with(
            &mut self,
            features: &crate::capabilities::ClientFeatures,
            diagnostics: Vec<Diagnostic>,
        ) {
            let selected = selected_interfaces(&self.physical, &self.open);
            self.client
                .publish(
                    &mut self.bytes,
                    &self.physical,
                    &self.physical.join("index.typ"),
                    &self.open,
                    features,
                    &selected,
                    diagnostics,
                )
                .unwrap();
        }

        pub(crate) fn pushed(&self) -> BTreeMap<String, Vec<EditorDiagnostic>> {
            let mut input = std::io::Cursor::new(self.bytes.clone());
            let mut by_uri: BTreeMap<String, Vec<EditorDiagnostic>> = BTreeMap::new();
            for parameters in reports(&mut input) {
                by_uri
                    .entry(parameters.uri.as_str().to_owned())
                    .or_default()
                    .extend(parameters.diagnostics);
            }
            by_uri
        }

        pub(crate) fn diagnostics(&self) -> Vec<EditorDiagnostic> {
            self.pushed().into_values().flatten().collect()
        }
    }

    /// A client that asked to be told which diagnostics are unnecessary and to receive the payload
    /// a correction reads back, and that asked for no versions: the report's own version is the
    /// server's record, not a wire field, so it must survive a client that wants none.
    fn tagged_features() -> crate::capabilities::ClientFeatures {
        crate::capabilities::ClientFeatures::new(
            &serde_json::from_value(serde_json::json!({
                "textDocument": {
                    "publishDiagnostics": {
                        "dataSupport": true,
                        "tagSupport": { "valueSet": [1] },
                    }
                }
            }))
            .unwrap(),
        )
    }
}
