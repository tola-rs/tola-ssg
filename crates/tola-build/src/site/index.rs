//! Complete site indexing and cross-output document validation.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tola_address::{OutputPath, asset_directory_index_alias, asset_url_from_output};
use tola_typst::HtmlDocumentInventory;

use super::{AddressRegistrationError, AddressSpace, HtmlPage, SiteAssetRoute};
use crate::compiler::documents::RealizedPage;
use crate::output::graph::{OutputGraph, OutputKind};

/// Realized documents, resource routes, and final HTML inventories of one site.
#[derive(Debug, Default)]
pub struct SiteIndex {
    address: AddressSpace,
    html_inventories: BTreeMap<OutputPath, Arc<HtmlDocumentInventory>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SiteIndexError {
    #[error(transparent)]
    Registration(#[from] AddressRegistrationError),
    #[error(
        "the root Bundle declares `{output}` as an HTML page, but this build generated no output for it; run the build again"
    )]
    DocumentOutputMissing { output: OutputPath },
    #[error(
        "the root Bundle declares `{output}` as an HTML page, but this build generated it as {}; run the build again",
        kind.display_name()
    )]
    DocumentOutputKind {
        output: OutputPath,
        kind: OutputKind,
    },
    #[error("`{output}` is an HTML page, but no Bundle document declares it; run the build again")]
    HtmlDocumentMissing { output: OutputPath },
}

impl SiteIndex {
    pub fn address(&self) -> &AddressSpace {
        &self.address
    }

    pub fn html_pages(&self) -> impl Iterator<Item = (&HtmlPage, &Arc<HtmlDocumentInventory>)> {
        self.html_inventories.iter().map(|(output, inventory)| {
            let document = self
                .address
                .page_by_output(output)
                .expect("HTML inventory has its validated document identity");
            (document, inventory)
        })
    }

    pub(crate) fn from_output_graph(
        graph: &OutputGraph,
        documents: impl IntoIterator<Item = RealizedPage>,
    ) -> Result<Self, SiteIndexError> {
        let outputs = graph
            .outputs()
            .iter()
            .map(|output| (output.path().clone(), output))
            .collect::<BTreeMap<_, _>>();
        let mut documents_by_output = BTreeMap::new();
        for document in documents {
            let output = document.page.output.clone();
            if documents_by_output
                .insert(output.clone(), document)
                .is_some()
            {
                return Err(
                    AddressRegistrationError::DocumentOutputAlreadyRegistered { output }.into(),
                );
            }
        }

        for output in documents_by_output.keys() {
            let candidate =
                outputs
                    .get(output)
                    .ok_or_else(|| SiteIndexError::DocumentOutputMissing {
                        output: output.clone(),
                    })?;
            if candidate.kind() != OutputKind::HtmlDocument {
                return Err(SiteIndexError::DocumentOutputKind {
                    output: output.clone(),
                    kind: candidate.kind(),
                });
            }
        }
        for output in graph.outputs() {
            if output.kind() == OutputKind::HtmlDocument
                && !documents_by_output.contains_key(output.path())
            {
                return Err(SiteIndexError::HtmlDocumentMissing {
                    output: output.path().clone(),
                });
            }
        }
        let mut address = AddressSpace::default();
        let mut html_inventories = BTreeMap::new();
        for (output, document) in documents_by_output {
            address.register_page(
                document.page,
                document
                    .inventory
                    .fragments()
                    .iter()
                    .map(|fragment| fragment.value().to_owned()),
            )?;
            html_inventories.insert(output, document.inventory);
        }
        let mut directory_indexes = Vec::new();
        for output in graph
            .outputs()
            .iter()
            .filter(|output| output.kind() != OutputKind::HtmlDocument)
        {
            let url = asset_url_from_output(output.path());
            if let Some(alias) = asset_directory_index_alias(output.path()) {
                directory_indexes.push((alias, url.clone()));
            }
            address.register_asset_route(SiteAssetRoute {
                source: output.owner().configured_source().map(PathBuf::from),
                url,
                declaration: output.declaration().clone(),
            })?;
        }
        address.register_asset_directory_indexes(directory_indexes);
        Ok(Self {
            address,
            html_inventories,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::site::Resource;
    use tola_address::UrlPath;

    /// Typst compilation for the fixed `index.html` source is expensive, so every caller shares
    /// one extracted inventory.
    static HTML_INVENTORY: std::sync::LazyLock<std::sync::Arc<tola_typst::HtmlDocumentInventory>> =
        std::sync::LazyLock::new(|| {
            let directory = TempDir::new().unwrap();
            let entry = directory.path().join("site.typ");
            fs::write(&entry, "#document(\"index.html\")[Page]").unwrap();
            let world = tola_typst::TypstWorld::builder(&entry, directory.path())
                .with_local_cache()
                .no_fonts()
                .build(&tola_typst::BundleCancellation::default())
                .unwrap();
            let compilation = tola_typst::compile_bundle_world(
                &world,
                &tola_typst::BundleCancellation::default(),
            )
            .unwrap();
            compilation
                .documents()
                .next()
                .unwrap()
                .html_inventory(&tola_typst::BundleCancellation::default())
                .unwrap()
                .unwrap()
        });

    fn html_inventory() -> std::sync::Arc<tola_typst::HtmlDocumentInventory> {
        std::sync::Arc::clone(&HTML_INVENTORY)
    }

    fn bundle_outputs(
        entries: impl IntoIterator<Item = tola_typst::BundleEntry>,
    ) -> crate::output::graph::OutputGraphBuilder {
        let mut outputs = crate::output::graph::OutputGraphBuilder::new();
        crate::compiler::outputs::insert_bundle(&mut outputs, entries).unwrap();
        outputs
    }

    #[test]
    fn asset_routes_keep_configured_source() {
        let entries = [
            ("paper.pdf", tola_typst::BundleEntryKind::PdfDocument),
            ("preview.png", tola_typst::BundleEntryKind::PngDocument),
            ("diagram.svg", tola_typst::BundleEntryKind::SvgDocument),
            ("raw.bin", tola_typst::BundleEntryKind::Asset),
        ]
        .map(|(path, kind)| {
            tola_typst::BundleEntry::new(
                typst::syntax::VirtualPath::new(path).unwrap(),
                kind,
                path.as_bytes().to_vec(),
            )
        });
        let mut outputs = bundle_outputs(entries);
        outputs
            .insert_configured_asset(
                "static/logo.svg",
                tola_address::OutputPath::parse("assets/logo.svg").unwrap(),
                crate::output::semantics::OutputDeclaration::from_filesystem_source(
                    std::path::Path::new("static/logo.svg"),
                ),
                b"logo".to_vec(),
            )
            .unwrap();
        let graph = outputs.finish();
        let snapshot = SiteIndex::from_output_graph(&graph, []).unwrap();

        for (url, source, media_type) in [
            (
                "/paper.pdf",
                None,
                crate::output::semantics::ResponseMediaType::PDF,
            ),
            (
                "/preview.png",
                None,
                crate::output::semantics::ResponseMediaType::PNG,
            ),
            (
                "/diagram.svg",
                None,
                crate::output::semantics::ResponseMediaType::SVG,
            ),
            (
                "/raw.bin",
                None,
                crate::output::semantics::ResponseMediaType::OCTET_STREAM,
            ),
            (
                "/assets/logo.svg",
                Some(PathBuf::from("static/logo.svg")),
                crate::output::semantics::ResponseMediaType::SVG,
            ),
        ] {
            let url = UrlPath::parse(url).unwrap();
            let Some(Resource::Asset { route }) = snapshot.address().get_by_url(&url) else {
                panic!("missing asset route {url}");
            };
            assert_eq!(route.source, source, "configured source for {url}");
            assert_eq!(
                route.declaration().media_type(),
                &media_type,
                "producer media type for {url}"
            );
        }
    }

    #[test]
    fn html_output_needs_document_identity() {
        let output = tola_address::OutputPath::parse("index.html").unwrap();
        let entry = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("index.html").unwrap(),
            tola_typst::BundleEntryKind::HtmlDocument,
            b"page".to_vec(),
        );
        let graph = bundle_outputs([entry]).finish();

        assert!(matches!(
            SiteIndex::from_output_graph(&graph, []),
            Err(SiteIndexError::HtmlDocumentMissing { output: missing })
                if missing == output
        ));
    }

    #[test]
    fn asset_claimed_document_url_rejected() {
        let output = tola_address::OutputPath::parse("page.html").unwrap();
        let entry = tola_typst::BundleEntry::new(
            typst::syntax::VirtualPath::new("page.html").unwrap(),
            tola_typst::BundleEntryKind::HtmlDocument,
            b"page".to_vec(),
        );
        let mut outputs = bundle_outputs([entry]);
        outputs
            .insert_configured_asset(
                "static/shared.bin",
                tola_address::OutputPath::parse("shared.bin").unwrap(),
                crate::output::semantics::OutputDeclaration::from_filesystem_source(
                    std::path::Path::new("static/shared.bin"),
                ),
                b"asset".to_vec(),
            )
            .unwrap();
        let graph = outputs.finish();
        let document = RealizedPage {
            page: HtmlPage {
                permalink: UrlPath::parse("/shared.bin").unwrap(),
                output,
                properties: typst::model::DocumentInfo::default(),
                sources: Vec::new(),
            },
            inventory: html_inventory(),
        };

        assert!(matches!(
            SiteIndex::from_output_graph(&graph, [document]),
            Err(SiteIndexError::Registration(
                AddressRegistrationError::UrlAlreadyRegistered { url }
            )) if url == UrlPath::parse("/shared.bin").unwrap()
        ));
    }
}
