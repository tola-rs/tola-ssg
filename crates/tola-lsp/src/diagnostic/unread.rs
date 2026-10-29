//! The unread imports one document's own text leaves, and the corrections a client reads from
//! them.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{Diagnostic as EditorDiagnostic, Uri};
use tola_diagnostic::{Diagnostic, DiagnosticCause, Severity, UnreadRemoval};
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::{self, ImportRemoval, SourceNames};

use crate::sentence::quoted;
use crate::sources::OpenSources;

use super::projection::{
    DiagnosticTexts, compiler_source_range, editor_diagnostic, protocol_source_range,
};
use super::store::ClientDiagnostics;

impl ClientDiagnostics {
    /// The unused imports one document's correction removes.
    ///
    /// `revision` is the revision this connection is checking: a completed check of that revision
    /// returned the index the site's selections are read from. A correction asked before one did
    /// reports only what a document's own scope proves — unless a statement's bindings the file's
    /// own scope holds need that index, which is [`UnreadImports::NeedsSelection`]: those are
    /// another source's to select, and no answer about them is honest without the site's own.
    pub(crate) fn unused_imports(
        &self,
        revision: u64,
        root: &Path,
        open: &OpenSources,
        uri: &Uri,
        source: &Source,
    ) -> Result<UnreadImports> {
        let requested = crate::uri::to_site_path(uri.as_str())?;
        let requested = tola_build::filesystem::normalize_existing_prefix(&requested);
        let mut texts = DiagnosticTexts::new(open);
        let unread = unread_documents(open, Some(source));
        let unproven = names::SelectedInterfaces {
            unresolved: true,
            selected: Default::default(),
        };
        let selection = self.selection_for(revision, root);
        let selected = match (&selection, unread.needs_selection) {
            (Some(selected), _) => selected.as_ref(),
            (None, true) => return Ok(UnreadImports::NeedsSelection),
            (None, false) => &unproven,
        };
        let mut answered = Vec::new();
        for diagnostic in unread_import_diagnostics(unread.documents, selected) {
            // Nothing this call builds has a located help or trace, so nothing folds.
            let projected =
                editor_diagnostic(diagnostic, root, &requested, &self.root, false, &mut texts)?;
            let Some((path, diagnostic)) = projected else {
                continue;
            };
            if tola_build::filesystem::normalize_existing_prefix(&path) == requested {
                answered.push(diagnostic);
            }
        }
        Ok(UnreadImports::Answered(answered))
    }
}

/// What one document's unused imports answer with.
#[derive(Debug, PartialEq)]
pub(crate) enum UnreadImports {
    /// The unread imports this connection could judge, as the editor reads them.
    Answered(Vec<EditorDiagnostic>),
    /// A statement whose bindings the file's own scope holds needs the site's selections, and this
    /// connection holds no index of them for the revision the request reads.
    NeedsSelection,
}

/// One document holding unread imports: the path a report addresses, its parsed names, and the
/// unread statements its own text leaves.
pub(super) struct UnreadDocument {
    path: String,
    names: Arc<SourceNames>,
    unread: Vec<names::UnreadImport>,
}

/// Every document whose own text leaves an unread import, and whether any of those statements is
/// another source's to select.
pub(super) struct UnreadDocuments {
    /// One entry per document holding at least one unread import.
    pub(super) documents: Vec<UnreadDocument>,
    /// Whether a statement's bindings the file's own scope holds is another source's to select:
    /// only the site's own selection index can tell one unread from one another source reads.
    pub(super) needs_selection: bool,
}

/// The unread imports every document the connection holds leaves.
///
/// An open document is read through the index the connection already parsed; a `requested` source
/// the open table does not hold is parsed here instead.
pub(super) fn unread_documents(open: &OpenSources, requested: Option<&Source>) -> UnreadDocuments {
    let mut documents = Vec::new();
    for parsed in open.view().iter() {
        let path = parsed.source.id().vpath().get_without_slash().to_owned();
        let names = parsed.names();
        let unread = names.unread_imports();
        if unread.is_empty() {
            continue;
        }
        documents.push(UnreadDocument {
            path,
            names,
            unread,
        });
    }
    if let Some(source) = requested
        && open
            .view()
            .iter()
            .all(|parsed| parsed.source.id() != source.id())
    {
        let names = Arc::new(SourceNames::new(source.clone()));
        let unread = names.unread_imports();
        if !unread.is_empty() {
            documents.push(UnreadDocument {
                path: source.id().vpath().get_without_slash().to_owned(),
                names,
                unread,
            });
        }
    }
    let needs_selection = documents
        .iter()
        .any(|document| document.unread.iter().any(|unread| unread.file_scope));
    UnreadDocuments {
        documents,
        needs_selection,
    }
}

/// The unread imports of these documents that `selected` licenses.
///
/// A statement whose bindings the file's own scope holds is judged against the site's selections;
/// an index that proves nothing withholds those statements rather than judging them from one that
/// does not describe the documents this call read.
pub(super) fn unread_import_diagnostics(
    documents: Vec<UnreadDocument>,
    selected: &names::SelectedInterfaces,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for document in documents {
        let lines = document.names.source().lines();
        for unread in document.names.reportable_unread_imports(selected) {
            let Some(statement) = crate::position::utf16_range(lines, unread.statement.clone())
            else {
                continue;
            };
            // The location has the compiler's line model, which a publish projects into the
            // protocol's; the removal the client echoes back stays protocol.
            let Some(compiler_statement) =
                tola_typst_syntax::position::utf16_range(lines, unread.statement.clone())
            else {
                continue;
            };
            let removal = match &unread.removal {
                ImportRemoval::Statement => UnreadRemoval::Statement {
                    line: statement.start.line as usize + 1,
                },
                ImportRemoval::Spans(spans) => UnreadRemoval::Spans {
                    ranges: spans
                        .iter()
                        .filter_map(|span| {
                            crate::position::utf16_range(lines, span.clone())
                                .map(protocol_source_range)
                        })
                        .collect(),
                },
            };
            diagnostics.push(
                Diagnostic::at_path(
                    tola_diagnostic::codes::check::UNUSED_IMPORT,
                    Severity::Warning,
                    document.path.clone(),
                    format!("{} imported but unused", quoted(&unread.names)),
                )
                .with_source_range(compiler_source_range(compiler_statement))
                .with_cause(DiagnosticCause::UnreadImport {
                    names: unread.names,
                    removal,
                }),
            );
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use crate::diagnostic::tests::Site;
    use lsp_types::{DiagnosticSeverity, DiagnosticTag, NumberOrString};

    #[test]
    fn unused_import_reaches_client_unnecessary() {
        let mut site = Site::new(&[("content/helpers.typ", "#let helper = 1\n#let orphan = 2\n")]);
        site.open(
            "content/partial.typ",
            "#import \"helpers.typ\": helper, orphan\n#helper\n",
        );
        site.check();
        let mut reported = Vec::new();
        for (uri, diagnostics) in site.pushed() {
            if diagnostics.is_empty() {
                continue;
            }
            reported.push((uri, diagnostics));
        }
        let [(uri, diagnostics)] = reported.as_slice() else {
            panic!("one document has the import: {reported:?}");
        };
        assert!(uri.ends_with("content/partial.typ"), "{uri}");
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("one unused import: {diagnostics:?}");
        };
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("check.unused_import".into()))
        );
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diagnostic.tags, Some(vec![DiagnosticTag::UNNECESSARY]));
        assert_eq!(diagnostic.message, "`orphan` imported but unused");
        let data = diagnostic.data.clone().expect("a machine-readable payload");
        assert_eq!(data["kind"], "unread-import");
        assert_eq!(data["names"], serde_json::json!(["orphan"]));
        assert_eq!(data["removal"]["kind"], "spans");
        let range = &data["removal"]["ranges"][0];
        assert_eq!(range["start"]["line"], 0);
        assert!(range["end"]["character"].as_u64() > range["start"]["character"].as_u64());
    }
}
