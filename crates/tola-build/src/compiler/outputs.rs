//! Convert Bundle exports into producer-owned output files.

use crate::output::graph::{OutputFile, OutputGraphBuilder, OutputGraphError};
use crate::output::owner::OutputOwner;
use crate::output::semantics::OutputDeclaration;
use tola_address::OutputPath;

pub(crate) fn insert_bundle(
    outputs: &mut OutputGraphBuilder,
    entries: impl IntoIterator<Item = tola_typst::BundleEntry>,
) -> Result<(), OutputGraphError> {
    let owner = OutputOwner::bundle("site-program");
    for entry in entries {
        let (raw_path, entry_kind, bytes, digest) = entry.into_parts();
        let path = OutputPath::parse(raw_path.get_without_slash()).map_err(|source| {
            OutputGraphError::InvalidPath {
                raw: raw_path.get_with_slash().to_owned(),
                owner: owner.clone(),
                source,
            }
        })?;
        let declaration = declaration_for_entry(entry_kind, &path);
        outputs.insert(OutputFile::from_byte_owner_with_digest(
            path,
            declaration,
            owner.clone(),
            bytes,
            digest,
        ))?;
    }
    Ok(())
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
    use crate::output::graph::{OutputGraph, OutputKind};

    fn bundle_graph(entries: impl IntoIterator<Item = tola_typst::BundleEntry>) -> OutputGraph {
        let mut graph = OutputGraphBuilder::new();
        insert_bundle(&mut graph, entries).unwrap();
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
