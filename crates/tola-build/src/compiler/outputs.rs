//! Convert Bundle exports into producer-owned output files.

use crate::output::graph::{
    OutputFile, OutputGraph, OutputGraphBuilder, OutputGraphError, OutputKind,
};
use crate::output::owner::OutputOwner;
use crate::output::semantics::OutputDeclaration;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use thiserror::Error;
use tola_address::{OutputPath, OutputPathError};

#[derive(Debug, Error)]
pub(crate) enum BundleOutputError {
    #[error(transparent)]
    Graph(#[from] OutputGraphError),
    #[error(
        "the Bundle exported {kind} `{path}`, but Tola did not recognize it as a site document"
    )]
    DocumentIdentityMissing { path: OutputPath, kind: OutputKind },
    #[error("the Bundle exported the asset `{path}`, but Tola expected a site document there")]
    AssetHasDocumentIdentity { path: OutputPath },
    #[error("Tola expected the Bundle to export `{path}`, but it exported no output for it")]
    DocumentOutputMissing { path: OutputPath },
    #[error("the Bundle now exports {kind} `{path}`, which the previous build did not")]
    ReexportUnexpectedOutput { path: OutputPath, kind: OutputKind },
    #[error("`{path}` is now {current}, but the previous build exported it as {previous}")]
    ReexportOutputKind {
        path: OutputPath,
        previous: OutputKind,
        current: OutputKind,
    },
    #[error("the Bundle no longer exports {kind} `{path}`")]
    ReexportOutputOmitted { path: OutputPath, kind: OutputKind },
    #[error("`{raw}` is not a valid output path when reusing a previous build: {source}")]
    ReexportInvalidPath {
        raw: String,
        #[source]
        source: OutputPathError,
    },
    #[error("`{path}` is {kind}, but the previous build recorded {owner} as its source")]
    ReexportOwnerMismatch {
        path: OutputPath,
        kind: OutputKind,
        owner: OutputOwner,
    },
}

pub(crate) fn insert_bundle(
    outputs: &mut OutputGraphBuilder,
    entries: impl IntoIterator<Item = tola_typst::BundleEntry>,
    producer: impl Into<Arc<str>>,
    document_outputs: &BTreeSet<OutputPath>,
) -> Result<(), BundleOutputError> {
    let producer = producer.into();
    let owner = OutputOwner::bundle(Arc::clone(&producer));
    let mut exported_documents = BTreeSet::new();
    for entry in entries {
        let (raw_path, entry_kind, bytes, _) = entry.into_parts();
        let (path, declaration) = entry_output(&raw_path, entry_kind).map_err(|source| {
            OutputGraphError::InvalidPath {
                raw: raw_path.get_with_slash().to_owned(),
                owner: owner.clone(),
                source,
            }
        })?;
        let kind = declaration.kind();
        if kind != OutputKind::Asset {
            if !document_outputs.contains(&path) {
                return Err(BundleOutputError::DocumentIdentityMissing {
                    path: path.clone(),
                    kind,
                });
            }
            exported_documents.insert(path.clone());
        } else {
            if document_outputs.contains(&path) {
                return Err(BundleOutputError::AssetHasDocumentIdentity { path });
            }
        }
        outputs.insert(OutputFile::from_byte_owner(
            path,
            declaration,
            owner.clone(),
            bytes,
        ))?;
    }
    if let Some(path) = document_outputs
        .iter()
        .find(|path| !exported_documents.contains(*path))
    {
        return Err(BundleOutputError::DocumentOutputMissing { path: path.clone() });
    }
    Ok(())
}

pub(crate) fn insert_reexported_bundle(
    outputs: &mut OutputGraphBuilder,
    entries: impl IntoIterator<Item = tola_typst::BundleEntry>,
    previous: &OutputGraph,
) -> Result<(), BundleOutputError> {
    let previous_outputs = previous
        .outputs()
        .iter()
        .map(|output| (output.path().clone(), output))
        .collect::<BTreeMap<_, _>>();
    let mut reexported = BTreeSet::new();

    for entry in entries {
        let (raw_path, entry_kind, bytes, _) = entry.into_parts();
        let (path, declaration) = entry_output(&raw_path, entry_kind).map_err(|source| {
            BundleOutputError::ReexportInvalidPath {
                raw: raw_path.get_with_slash().to_owned(),
                source,
            }
        })?;
        let kind = declaration.kind();
        let previous_output = previous_outputs.get(&path).ok_or_else(|| {
            BundleOutputError::ReexportUnexpectedOutput {
                path: path.clone(),
                kind,
            }
        })?;
        if previous_output.kind() != kind {
            return Err(BundleOutputError::ReexportOutputKind {
                path,
                previous: previous_output.kind(),
                current: kind,
            });
        }
        let has_bundle_owner = matches!(previous_output.owner(), OutputOwner::Bundle { .. });
        if !has_bundle_owner {
            return Err(BundleOutputError::ReexportOwnerMismatch {
                path,
                kind,
                owner: previous_output.owner().clone(),
            });
        }
        reexported.insert(path.clone());
        outputs.insert(OutputFile::from_byte_owner(
            path,
            declaration,
            previous_output.owner().clone(),
            bytes,
        ))?;
    }

    if let Some(output) = previous
        .outputs()
        .iter()
        .find(|output| !reexported.contains(output.path()))
    {
        return Err(BundleOutputError::ReexportOutputOmitted {
            path: output.path().clone(),
            kind: output.kind(),
        });
    }
    Ok(())
}

/// Parse one Bundle entry's logical output identity and its output declaration.
fn entry_output(
    raw_path: &typst::syntax::VirtualPath,
    entry_kind: tola_typst::BundleEntryKind,
) -> Result<(OutputPath, OutputDeclaration), OutputPathError> {
    let path = OutputPath::parse(raw_path.get_without_slash())?;
    let declaration = declaration_for_entry(entry_kind, &path);
    Ok((path, declaration))
}

fn declaration_for_entry(
    kind: tola_typst::BundleEntryKind,
    path: &OutputPath,
) -> OutputDeclaration {
    match kind {
        tola_typst::BundleEntryKind::HtmlDocument => OutputDeclaration::html_document(),
        tola_typst::BundleEntryKind::PdfDocument => OutputDeclaration::pdf_document(),
        tola_typst::BundleEntryKind::PngDocument => OutputDeclaration::png_document(),
        tola_typst::BundleEntryKind::SvgDocument => OutputDeclaration::svg_document(),
        tola_typst::BundleEntryKind::Asset => OutputDeclaration::opaque_from_output_path(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle_graph(entries: impl IntoIterator<Item = tola_typst::BundleEntry>) -> OutputGraph {
        let entries = entries.into_iter().collect::<Vec<_>>();
        let document_outputs = entries
            .iter()
            .filter(|entry| entry.kind().is_document())
            .map(|entry| OutputPath::parse(entry.path().get_without_slash()).unwrap())
            .collect();
        let mut graph = OutputGraphBuilder::new();
        crate::compiler::outputs::insert_bundle(&mut graph, entries, "site.typ", &document_outputs)
            .unwrap();
        graph.finish()
    }

    #[test]
    fn mixed_bundle_entries_become_outputs() {
        let entries = vec![
            tola_typst::BundleEntry::new(
                typst::syntax::VirtualPath::new("index.html").unwrap(),
                tola_typst::BundleEntryKind::HtmlDocument,
                b"home".to_vec(),
            ),
            tola_typst::BundleEntry::new(
                typst::syntax::VirtualPath::new("search.json").unwrap(),
                tola_typst::BundleEntryKind::Asset,
                b"{}".to_vec(),
            ),
        ];
        let graph = bundle_graph(entries);
        assert_eq!(graph.outputs().len(), 2);
        assert_eq!(graph.outputs()[0].path().as_str(), "index.html");
        assert_eq!(graph.outputs()[1].path().as_str(), "search.json");
        assert_eq!(graph.outputs()[1].bytes(), b"{}");
    }

    #[test]
    fn document_entries_need_identities() {
        let document = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("paper.pdf").unwrap(),
            tola_typst::BundleEntryKind::PdfDocument,
            b"pdf".to_vec(),
        );
        let mut missing = OutputGraphBuilder::new();
        assert!(matches!(
            crate::compiler::outputs::insert_bundle(
                &mut missing,
                [document],
                "site.typ",
                &BTreeSet::new()
            ),
            Err(BundleOutputError::DocumentIdentityMissing { .. })
        ));

        let raw = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("raw.bin").unwrap(),
            tola_typst::BundleEntryKind::Asset,
            b"raw".to_vec(),
        );
        let unexpected = [OutputPath::parse("raw.bin").unwrap()]
            .into_iter()
            .collect();
        let mut outputs = OutputGraphBuilder::new();
        assert!(matches!(
            crate::compiler::outputs::insert_bundle(&mut outputs, [raw], "site.typ", &unexpected),
            Err(BundleOutputError::AssetHasDocumentIdentity { .. })
        ));
    }

    #[test]
    fn reexport_keeps_the_validated_owner() {
        let output = OutputPath::parse("paper.pdf").unwrap();
        let document_outputs = [output.clone()].into_iter().collect();
        let first_entry = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("paper.pdf").unwrap(),
            tola_typst::BundleEntryKind::PdfDocument,
            b"first".to_vec(),
        );
        let mut first = OutputGraphBuilder::new();
        crate::compiler::outputs::insert_bundle(
            &mut first,
            [first_entry],
            "site.typ",
            &document_outputs,
        )
        .unwrap();
        let first = first.finish();

        let second_entry = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("paper.pdf").unwrap(),
            tola_typst::BundleEntryKind::PdfDocument,
            b"second".to_vec(),
        );
        let mut second = OutputGraphBuilder::new();
        crate::compiler::outputs::insert_reexported_bundle(&mut second, [second_entry], &first)
            .unwrap();
        let second = second.finish();

        assert_eq!(second.outputs()[0].owner(), first.outputs()[0].owner());
        assert_eq!(second.outputs()[0].bytes(), b"second");
    }

    #[test]
    fn entry_kind_outranks_the_filename() {
        let entries = vec![
            tola_typst::BundleEntry::new(
                typst::syntax::VirtualPath::new("page.bin").unwrap(),
                tola_typst::BundleEntryKind::HtmlDocument,
                Vec::new(),
            ),
            tola_typst::BundleEntry::new(
                typst::syntax::VirtualPath::new("tags/index.html").unwrap(),
                tola_typst::BundleEntryKind::Asset,
                Vec::new(),
            ),
        ];
        let graph = bundle_graph(entries);
        assert_eq!(graph.outputs()[0].kind(), OutputKind::HtmlDocument);
        assert_eq!(graph.outputs()[0].path().as_str(), "page.bin");
        assert_eq!(graph.outputs()[1].kind(), OutputKind::Asset);
        assert_eq!(graph.outputs()[1].path().as_str(), "tags/index.html");
    }
}
