//! Candidate outputs produced before the output is written.

pub mod files;
pub mod graph;
pub mod manifest;
pub mod owner;
pub(crate) mod path;
pub mod revision;
mod root;
pub mod semantics;
pub mod summary;

pub use graph::{GeneratedFile, GeneratedFileError};
pub use summary::PageAvailability;

pub(crate) use root::{OutputWrite, resolve_site_output_root_with_inputs};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::graph::{OutputFile, OutputGraph, OutputGraphBuilder, OutputKind};
    use super::owner::OutputOwner;
    use super::semantics::OutputDeclaration;
    use tola_address::OutputPath;

    /// One output whose owner kind agrees with its declaration.
    pub(crate) fn output_file(
        path: &str,
        declaration: OutputDeclaration,
        bytes: &[u8],
    ) -> OutputFile {
        let path = OutputPath::parse(path).unwrap();
        let owner = match declaration.kind() {
            OutputKind::Asset => OutputOwner::system("test"),
            OutputKind::HtmlDocument
            | OutputKind::PdfDocument
            | OutputKind::PngDocument
            | OutputKind::SvgDocument => OutputOwner::bundle("test"),
        };
        OutputFile::new(path, declaration, owner, bytes.to_vec())
    }

    pub(crate) fn insert_output(
        graph: &mut OutputGraphBuilder,
        path: &str,
        declaration: OutputDeclaration,
        bytes: &[u8],
    ) {
        graph.insert(output_file(path, declaration, bytes)).unwrap();
    }

    pub(crate) fn output_graph(outputs: impl IntoIterator<Item = OutputFile>) -> OutputGraph {
        let mut graph = OutputGraphBuilder::new();
        for output in outputs {
            graph.insert(output).unwrap();
        }
        graph.finish()
    }

    pub(crate) fn site_lock(root: &Path) -> crate::build::SiteBuildLock {
        crate::build::SiteBuildLock::acquire_at_root(
            root,
            &crate::cancellation::BuildCancellation::default(),
            || panic!("uncontended site lock waited"),
        )
        .unwrap()
    }
}
