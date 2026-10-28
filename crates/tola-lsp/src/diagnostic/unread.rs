//! The unread imports one document's own text leaves, and the corrections a client reads from
//! them.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{Diagnostic as EditorDiagnostic, Uri};
use tola_build::diagnostic::{Diagnostic, DiagnosticCause, Severity, UnreadRemoval};
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
        let (documents, file_scope) = unread_documents(open, Some(source));
        let unproven = names::SelectedInterfaces {
            unresolved: true,
            selected: Default::default(),
        };
        let selection = self.selection_for(revision, root);
        let selected = match (&selection, file_scope) {
            (Some(selected), _) => selected.as_ref(),
            (None, false) => &unproven,
            (None, true) => return Ok(UnreadImports::NeedsSelection),
        };
        let diagnostics = unread_import_diagnostics(documents, selected)
            .into_iter()
            // Nothing this call builds has a located help or trace, so nothing folds.
            .map(|diagnostic| {
                editor_diagnostic(diagnostic, root, &requested, &self.root, false, &mut texts)
            })
            .filter_map(|projected| match projected {
                Ok((path, diagnostic))
                    if tola_build::filesystem::normalize_existing_prefix(&path) == requested =>
                {
                    Some(Ok(diagnostic))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(UnreadImports::Answered(diagnostics))
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

/// Every document's unread imports, and whether any statement among them needs the site's
/// selections: a binding the file's own scope holds is another source's to select, while a nested
/// one reaches no other source.
///
/// The connection parses each open document once, so this reads the index it already holds.
pub(super) fn unread_documents(
    open: &OpenSources,
    requested: Option<&Source>,
) -> (Vec<UnreadDocument>, bool) {
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
    let file_scope = documents
        .iter()
        .any(|document| document.unread.iter().any(|unread| unread.file_scope));
    (documents, file_scope)
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
        for unread in document.names.reportable_unread_imports(selected) {
            let lines = document.names.source().lines();
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
                    tola_build::codes::check::UNUSED_IMPORT,
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
    fn unused_import_reaches_the_client_as_unnecessary() {
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
