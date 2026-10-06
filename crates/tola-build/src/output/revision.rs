//! Immutable outputs visible to one site revision.

use rustc_hash::FxHashMap;

use super::graph::{OutputFile, OutputGraph};
use super::manifest::SiteManifest;
use tola_address::OutputPath;

/// Indexed output bytes, declarations, and manifest for one immutable site revision.
#[derive(Debug, Clone)]
pub struct OutputRevision {
    outputs: OutputGraph,
    positions: FxHashMap<OutputPath, usize>,
    manifest: SiteManifest,
}

impl OutputRevision {
    /// Index one complete graph without copying its file bytes.
    pub fn from_graph(graph: &OutputGraph) -> Self {
        let outputs = graph.clone();
        let positions = outputs
            .outputs()
            .iter()
            .enumerate()
            .map(|(index, output)| (output.path().clone(), index))
            .collect();
        let manifest = SiteManifest::from_graph(&outputs);
        Self {
            outputs,
            positions,
            manifest,
        }
    }

    /// Build the next complete revision while retaining unchanged output storage.
    pub fn from_graph_reusing(previous: &Self, graph: &OutputGraph) -> Self {
        Self::from_graph(&graph.sharing_unchanged_bytes(|path| previous.output(path)))
    }

    pub fn output(&self, path: &OutputPath) -> Option<&OutputFile> {
        self.positions
            .get(path)
            .map(|index| &self.outputs.outputs()[*index])
    }

    /// Look up an exact logical path without allocating or parsing an [`OutputPath`].
    pub fn output_by_path_text(&self, path: &str) -> Option<&OutputFile> {
        self.positions
            .get(path)
            .map(|index| &self.outputs.outputs()[*index])
    }

    pub fn outputs(&self) -> &[OutputFile] {
        self.outputs.outputs()
    }

    /// Exclusive directory ownership retained from the complete candidate.
    pub fn root_ownerships(&self) -> &[super::owner::OutputRootOwnership] {
        self.outputs.root_ownerships()
    }

    pub fn page_availability(&self) -> crate::output::PageAvailability {
        crate::output::PageAvailability::from_outputs(self.outputs.outputs())
    }

    pub fn manifest(&self) -> &SiteManifest {
        &self.manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::graph::{OutputGraphBuilder, OutputKind};
    use crate::output::owner::OutputOwner;
    use crate::output::semantics::{OutputDeclaration, ResponseMediaType};
    use crate::output::tests::{insert_output, output_file, output_graph};

    #[test]
    fn manifest_matches_served_output() {
        let mut graph = OutputGraphBuilder::new();
        insert_output(
            &mut graph,
            "posts/rust/index.html",
            OutputDeclaration::html_document(),
            b"Rust",
        );
        let revision = OutputRevision::from_graph(&graph.finish());
        let path = OutputPath::parse("posts/rust/index.html").unwrap();
        let output = revision.output(&path).unwrap();

        assert_eq!(output.bytes(), b"Rust");
        let representation =
            crate::output::manifest::RepresentationId::from_output(output).to_hex();
        assert!(
            revision
                .manifest()
                .matches_representation(&path, &representation)
        );
        assert_eq!(output.kind(), OutputKind::HtmlDocument);
        assert!(std::ptr::eq(
            output,
            revision
                .output_by_path_text("posts/rust/index.html")
                .unwrap(),
        ));
    }

    #[test]
    fn unchanged_outputs_keep_their_storage() {
        fn graph(stable: &[u8], changed: &[u8]) -> OutputGraph {
            output_graph([
                output_file(
                    "stable.css",
                    OutputDeclaration::opaque(ResponseMediaType::CSS),
                    stable,
                ),
                output_file("index.html", OutputDeclaration::html_document(), changed),
            ])
        }

        let first = OutputRevision::from_graph(&graph(b"stable", b"before"));
        let next = OutputRevision::from_graph_reusing(&first, &graph(b"stable", b"after"));
        let stable = OutputPath::parse("stable.css").unwrap();
        let changed = OutputPath::parse("index.html").unwrap();

        assert!(std::ptr::eq(
            first.output(&stable).unwrap().bytes().as_ptr(),
            next.output(&stable).unwrap().bytes().as_ptr(),
        ));
        assert!(!std::ptr::eq(
            first.output(&changed).unwrap().bytes().as_ptr(),
            next.output(&changed).unwrap().bytes().as_ptr(),
        ));
        assert_eq!(next.output(&changed).unwrap().bytes(), b"after");
    }

    #[test]
    fn candidate_ownership_replaces_previous() {
        fn owned_graph(owner: OutputOwner) -> OutputGraph {
            let mut graph = OutputGraphBuilder::new();
            for root in ["search", "empty"] {
                graph
                    .own_root(super::super::owner::OutputRootOwnership::new(
                        OutputPath::parse(root).unwrap(),
                        owner.clone(),
                    ))
                    .unwrap();
            }
            graph
                .insert(OutputFile::new(
                    OutputPath::parse("search/index.json").unwrap(),
                    OutputDeclaration::opaque(ResponseMediaType::JSON),
                    owner,
                    b"{}".to_vec(),
                ))
                .unwrap();
            graph.finish()
        }

        let first = OutputRevision::from_graph(&owned_graph(OutputOwner::command(0, "first")));
        let owner = OutputOwner::command(1, "current");
        let candidate = owned_graph(owner.clone());
        let next = OutputRevision::from_graph_reusing(&first, &candidate);

        assert_eq!(next.outputs()[0].owner(), &owner);
        assert_eq!(next.root_ownerships(), candidate.root_ownerships());
        assert_eq!(next.manifest(), &SiteManifest::from_graph(&candidate));
    }

    #[test]
    fn declaration_change_drops_shared_storage() {
        fn graph(declaration: OutputDeclaration) -> OutputGraph {
            output_graph([output_file("styles/app", declaration, &[b'x'; 1024])])
        }

        let first = OutputRevision::from_graph(&graph(OutputDeclaration::opaque(
            ResponseMediaType::OCTET_STREAM,
        )));
        let next = OutputRevision::from_graph_reusing(
            &first,
            &graph(OutputDeclaration::from_filesystem_source(
                std::path::Path::new("styles/app.css"),
            )),
        );
        let path = OutputPath::parse("styles/app").unwrap();

        assert!(!std::ptr::eq(
            first.output(&path).unwrap().bytes().as_ptr(),
            next.output(&path).unwrap().bytes().as_ptr(),
        ));
        assert_eq!(
            next.output(&path).unwrap().declaration(),
            &OutputDeclaration::from_filesystem_source(std::path::Path::new("styles/app.css"))
        );
    }

    #[test]
    fn reuse_drops_outputs_the_candidate_omits() {
        let mut first_graph = OutputGraphBuilder::new();
        insert_output(
            &mut first_graph,
            "old.css",
            OutputDeclaration::opaque(ResponseMediaType::CSS),
            b"old",
        );
        let first = OutputRevision::from_graph(&first_graph.finish());

        let mut next_graph = OutputGraphBuilder::new();
        insert_output(
            &mut next_graph,
            "new.css",
            OutputDeclaration::opaque(ResponseMediaType::CSS),
            b"new",
        );
        let next = OutputRevision::from_graph_reusing(&first, &next_graph.finish());

        assert!(
            next.output(&OutputPath::parse("old.css").unwrap())
                .is_none()
        );
        assert_eq!(
            next.output(&OutputPath::parse("new.css").unwrap())
                .unwrap()
                .bytes(),
            b"new"
        );
    }
}
