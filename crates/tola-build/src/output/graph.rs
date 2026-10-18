//! Validated ownership graph for one site candidate.

use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;

use super::owner::OutputOwner;
use super::path::{OutputPathConflict, OutputPathIndex};
use super::semantics::OutputDeclaration;
use super::summary::OutputCounts;
use tola_address::{OutputPath, OutputPathError, portable_key_is_reserved};
use tola_diagnostic::{Diagnostic, Severity};
use tola_typst::ContentDigest;

/// Document-or-asset view derived from a candidate's producer declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputKind {
    HtmlDocument,
    PdfDocument,
    PngDocument,
    SvgDocument,
    Asset,
}

impl std::fmt::Display for OutputKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display_name())
    }
}

/// One immutable file produced by a validated site build.
#[derive(Debug, Clone)]
pub struct OutputFile {
    path: OutputPath,
    declaration: OutputDeclaration,
    owner: OutputOwner,
    bytes: OutputBytes,
    digest: ContentDigest,
}

/// A native producer's immutable file supplied before the output graph is sealed.
///
/// Supplied bytes remain opaque resources, not Bundle documents. Path reservations,
/// collisions, and incoming references are checked alongside other outputs;
/// file contents are not parsed as document structure.
#[derive(Debug, Clone)]
pub struct GeneratedFile(OutputFile);

/// A native producer must have a non-empty, unpadded name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a generated file needs a non-empty name with no surrounding spaces")]
pub struct GeneratedFileError;

impl GeneratedFile {
    /// Supply final bytes, inferring the response media type from the output extension.
    pub fn new(
        producer: impl Into<Arc<str>>,
        path: OutputPath,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<Self, GeneratedFileError> {
        let producer = producer.into();
        if producer.is_empty() || producer.trim() != producer.as_ref() {
            return Err(GeneratedFileError);
        }
        let declaration = OutputDeclaration::opaque_from_output_path(&path);
        Ok(Self(OutputFile::new(
            path,
            declaration,
            OutputOwner::Generated { producer },
            bytes,
        )))
    }

    /// Override response media without assigning a Typst document identity.
    pub fn with_media_type(mut self, media_type: super::semantics::ResponseMediaType) -> Self {
        self.0.declaration = OutputDeclaration::opaque(media_type);
        self
    }

    pub fn producer(&self) -> &str {
        let OutputOwner::Generated { producer } = &self.0.owner else {
            unreachable!("native file construction fixes its producer kind")
        };
        producer
    }

    pub fn path(&self) -> &OutputPath {
        self.0.path()
    }

    pub fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }

    pub fn media_type(&self) -> &super::semantics::ResponseMediaType {
        self.0.declaration.media_type()
    }

    pub(crate) fn into_file(self) -> OutputFile {
        self.0
    }
}

/// Immutable output bytes; clones share the allocation and outlive the build that produced them.
#[derive(Debug, Clone)]
pub struct OutputBytes(bytes::Bytes);

impl OutputBytes {
    fn as_slice(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl AsRef<[u8]> for OutputBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl OutputFile {
    pub(crate) fn new(
        path: OutputPath,
        declaration: OutputDeclaration,
        owner: OutputOwner,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        let bytes: Arc<[u8]> = bytes.into();
        Self::from_byte_owner(path, declaration, owner, bytes)
    }

    /// Retain an immutable byte owner without copying its allocation.
    pub(crate) fn from_byte_owner(
        path: OutputPath,
        declaration: OutputDeclaration,
        owner: OutputOwner,
        bytes: impl AsRef<[u8]> + Send + 'static,
    ) -> Self {
        let bytes = bytes::Bytes::from_owner(bytes);
        let digest = ContentDigest::of(&bytes);
        Self::from_owned_bytes(path, declaration, owner, bytes, digest)
    }

    /// Retain an immutable byte owner; `digest` must be the digest of every byte it yields.
    pub(crate) fn from_byte_owner_with_digest(
        path: OutputPath,
        declaration: OutputDeclaration,
        owner: OutputOwner,
        bytes: impl AsRef<[u8]> + Send + 'static,
        digest: ContentDigest,
    ) -> Self {
        Self::from_owned_bytes(
            path,
            declaration,
            owner,
            bytes::Bytes::from_owner(bytes),
            digest,
        )
    }

    fn from_owned_bytes(
        path: OutputPath,
        declaration: OutputDeclaration,
        owner: OutputOwner,
        bytes: bytes::Bytes,
        digest: ContentDigest,
    ) -> Self {
        Self {
            path,
            declaration,
            owner,
            digest,
            bytes: OutputBytes(bytes),
        }
    }

    #[inline]
    pub fn path(&self) -> &OutputPath {
        &self.path
    }

    #[inline]
    pub fn kind(&self) -> OutputKind {
        self.declaration.kind()
    }

    #[inline]
    pub fn declaration(&self) -> &OutputDeclaration {
        &self.declaration
    }

    #[inline]
    pub fn owner(&self) -> &OutputOwner {
        &self.owner
    }

    /// The same bytes and delivery metadata under a new producer.
    pub(crate) fn with_owner(&self, owner: OutputOwner) -> Self {
        Self::from_byte_owner_with_digest(
            self.path.clone(),
            self.declaration.clone(),
            owner,
            self.bytes_owner(),
            self.digest,
        )
    }

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Clone the immutable storage owner without copying its byte allocation.
    #[inline]
    pub fn bytes_owner(&self) -> OutputBytes {
        self.bytes.clone()
    }

    #[inline]
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }

    /// Shared storage is only an optimization; distinct ranges still need byte comparison.
    #[inline]
    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        std::ptr::eq(self.bytes.as_slice(), other.bytes.as_slice())
    }
}

impl OutputKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HtmlDocument => "html-document",
            Self::PdfDocument => "pdf-document",
            Self::PngDocument => "png-document",
            Self::SvgDocument => "svg-document",
            Self::Asset => "asset",
        }
    }

    /// Author-facing name; the serde spelling is the stable protocol form.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::HtmlDocument => "an HTML page",
            Self::PdfDocument => "a PDF document",
            Self::PngDocument => "a PNG document",
            Self::SvgDocument => "an SVG document",
            Self::Asset => "a file",
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum OutputGraphError {
    #[error("`{path}` and the directory `{tree}` are the same output")]
    TreeConflict {
        tree: OutputPath,
        tree_owner: Box<OutputOwner>,
        path: OutputPath,
        owner: Box<OutputOwner>,
    },
    #[error("`{raw}` is not a valid site output path for {owner}: {source}")]
    InvalidPath {
        raw: String,
        owner: OutputOwner,
        #[source]
        source: OutputPathError,
    },
    #[error("`{path}` is reserved for Tola's own files")]
    ReservedPath {
        path: OutputPath,
        owner: OutputOwner,
    },
    #[error(
        "`{first_path}` and `{second_path}` are the same output, from {first_owner} and {second_owner}: {conflict}"
    )]
    PathConflict {
        conflict: OutputPathConflict,
        first_path: OutputPath,
        first_owner: Box<OutputOwner>,
        second_path: OutputPath,
        second_owner: Box<OutputOwner>,
    },
    #[error("`{path}` is {}, which {owner} cannot produce", kind.display_name())]
    OutputOwnerKind {
        path: OutputPath,
        kind: OutputKind,
        owner: OutputOwner,
    },
}

impl OutputGraphError {
    /// The action the site author can take, for the diagnostic that reports this error.
    fn help(&self) -> &'static str {
        match self {
            Self::TreeConflict { .. } => "use a path that does not overlap that directory",
            Self::ReservedPath { .. } => "use a path outside `_tola`",
            Self::PathConflict { .. } => "give one of them a different path or route",
            Self::OutputOwnerKind { .. } => "declare it with `document(...)` in the root Bundle",
            Self::InvalidPath { .. } => "use a path relative to the site root",
        }
    }
}

/// Report one output conflict in the site author's own terms.
pub(crate) fn error_diagnostic(error: &anyhow::Error) -> Option<Diagnostic> {
    let cause = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<OutputGraphError>())?;
    Some(match cause {
        OutputGraphError::PathConflict {
            conflict,
            first_path,
            first_owner,
            second_path,
            second_owner,
        } => {
            // Two spellings that agree on every filesystem collide as one path, so both are worth
            // naming; identical spellings need naming once.
            let respelled = first_path != second_path;
            let message = if respelled {
                format!("`{first_path}` and `{second_path}` are the same output")
            } else {
                format!("`{first_path}` is published twice")
            };
            let mut diagnostic = Diagnostic::new(
                tola_diagnostic::codes::output::DUPLICATE,
                Severity::Error,
                message,
            )
            .with_note(format!("published by {first_owner} and {second_owner}"))
            .with_help(cause.help());
            if respelled {
                diagnostic = diagnostic.with_note(conflict.to_string());
            }
            diagnostic
        }
        OutputGraphError::TreeConflict {
            tree,
            tree_owner,
            path,
            owner,
        } => {
            let mut diagnostic = Diagnostic::new(
                tola_diagnostic::codes::site::OUTPUTS,
                Severity::Error,
                cause.to_string(),
            )
            .with_note(format!("published by {owner} and {tree_owner}"))
            .with_help(cause.help());
            // Two spellings that agree on every filesystem meet at one path, so the reader learns
            // why the file and the directory collide.
            if tree != path && tree.portable_key() == path.portable_key() {
                diagnostic = diagnostic.with_note(OutputPathConflict::SamePath.to_string());
            }
            diagnostic
        }
        other => Diagnostic::new(
            tola_diagnostic::codes::site::OUTPUTS,
            Severity::Error,
            other.to_string(),
        )
        .with_help(other.help()),
    })
}

/// Collects candidate files and checks path and producer conflicts before sealing.
#[derive(Debug, Clone, Default)]
pub(crate) struct OutputGraphBuilder {
    outputs: Vec<OutputFile>,
    paths: OutputPathIndex,
    root_ownerships: Vec<super::owner::OutputRootOwnership>,
    roots: OutputPathIndex,
}

/// `_tola` belongs to Tola; only Tola's own outputs may claim a path inside it.
fn reject_reserved_path(
    portable_key: &[String],
    path: &OutputPath,
    owner: &OutputOwner,
) -> Result<(), OutputGraphError> {
    if owner.is_system() || !portable_key_is_reserved(portable_key) {
        return Ok(());
    }
    Err(OutputGraphError::ReservedPath {
        path: path.clone(),
        owner: owner.clone(),
    })
}

impl OutputGraphBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, output: OutputFile) -> Result<(), OutputGraphError> {
        let portable_key = output.path.portable_key();
        if let Some(index) = self.roots.ownership_conflict(
            &portable_key,
            |index| self.root_ownerships[index].owner() == output.owner(),
            |_| false,
        ) {
            return Err(tree_file_conflict(&self.root_ownerships[index], &output));
        }
        // Only the root Bundle declares documents; every other producer supplies assets.
        let kind = output.kind();
        if kind != OutputKind::Asset && !matches!(&output.owner, OutputOwner::Bundle { .. }) {
            return Err(OutputGraphError::OutputOwnerKind {
                path: output.path,
                kind,
                owner: output.owner,
            });
        }
        reject_reserved_path(&portable_key, &output.path, &output.owner)?;

        if let Some((index, conflict)) = self.paths.conflict(&portable_key) {
            let existing = &self.outputs[index];
            return Err(OutputGraphError::PathConflict {
                conflict,
                first_path: existing.path.clone(),
                first_owner: Box::new(existing.owner.clone()),
                second_path: output.path,
                second_owner: Box::new(output.owner),
            });
        }

        let index = self.outputs.len();
        self.outputs.push(output);
        self.paths.insert(portable_key, index, |first, second| {
            self.outputs[first].owner() == self.outputs[second].owner()
        });
        Ok(())
    }

    pub(crate) fn insert_graph(&mut self, graph: &OutputGraph) -> Result<(), OutputGraphError> {
        for ownership in graph.root_ownerships.iter() {
            self.own_root(ownership.clone())?;
        }
        for output in graph.outputs() {
            self.insert(output.clone())?;
        }
        Ok(())
    }

    pub(crate) fn own_root(
        &mut self,
        ownership: super::owner::OutputRootOwnership,
    ) -> Result<(), OutputGraphError> {
        let key = ownership.root().portable_key();
        reject_reserved_path(&key, ownership.root(), ownership.owner())?;
        if let Some(index) = self.roots.ownership_conflict(&key, |_| false, |_| false) {
            let existing = &self.root_ownerships[index];
            return Err(OutputGraphError::TreeConflict {
                tree: existing.root().clone(),
                tree_owner: Box::new(existing.owner().clone()),
                path: ownership.root().clone(),
                owner: Box::new(ownership.owner().clone()),
            });
        }
        if let Some(index) = self.paths.ownership_conflict(
            &key,
            |_| false,
            |index| self.outputs[index].owner() == ownership.owner(),
        ) {
            return Err(tree_file_conflict(&ownership, &self.outputs[index]));
        }
        self.roots
            .insert(key, self.root_ownerships.len(), |_, _| true);
        self.root_ownerships.push(ownership);
        Ok(())
    }

    pub(crate) fn insert_configured_asset(
        &mut self,
        source: impl Into<std::path::PathBuf>,
        path: OutputPath,
        declaration: OutputDeclaration,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<(), OutputGraphError> {
        let owner = OutputOwner::configured_asset(source);
        self.insert(OutputFile::new(path, declaration, owner, bytes))
    }

    pub(crate) fn insert_system(
        &mut self,
        producer: impl Into<Arc<str>>,
        raw_path: &str,
        declaration: OutputDeclaration,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<(), OutputGraphError> {
        let owner = OutputOwner::system(producer);
        let path = OutputPath::parse(raw_path).map_err(|source| OutputGraphError::InvalidPath {
            raw: raw_path.to_owned(),
            owner: owner.clone(),
            source,
        })?;
        self.insert(OutputFile::new(path, declaration, owner, bytes))
    }

    pub(crate) fn outputs(&self) -> &[OutputFile] {
        &self.outputs
    }

    pub(crate) fn finish(self) -> OutputGraph {
        OutputGraph {
            outputs: self.outputs.into(),
            root_ownerships: self.root_ownerships.into(),
        }
    }
}

/// Immutable files with validated output paths and producer ownership.
///
/// Available from [`crate::build::SiteBuild`]. Clones share file storage.
#[derive(Debug, Clone)]
pub struct OutputGraph {
    outputs: Arc<[OutputFile]>,
    root_ownerships: Arc<[super::owner::OutputRootOwnership]>,
}

fn tree_file_conflict(
    ownership: &super::owner::OutputRootOwnership,
    output: &OutputFile,
) -> OutputGraphError {
    OutputGraphError::TreeConflict {
        tree: ownership.root().clone(),
        tree_owner: Box::new(ownership.owner().clone()),
        path: output.path().clone(),
        owner: Box::new(output.owner().clone()),
    }
}

impl OutputGraph {
    /// Reuse byte owners while preserving every validated path, declaration,
    /// and directory ownership in this graph.
    pub(super) fn sharing_unchanged_bytes<'a>(
        &self,
        mut previous: impl FnMut(&OutputPath) -> Option<&'a OutputFile>,
    ) -> Self {
        let outputs = self
            .outputs
            .iter()
            .map(|candidate| {
                let mut output = candidate.clone();
                if let Some(previous) = previous(candidate.path()).filter(|previous| {
                    previous.owner() == candidate.owner()
                        && previous.declaration() == candidate.declaration()
                        && previous.digest() == candidate.digest()
                        && (previous.shares_storage_with(candidate)
                            || previous.bytes() == candidate.bytes())
                }) {
                    output.bytes = previous.bytes.clone();
                }
                output
            })
            .collect::<Vec<_>>();
        Self {
            outputs: outputs.into(),
            root_ownerships: Arc::clone(&self.root_ownerships),
        }
    }

    /// Complete files in insertion order; producers insert deterministically, so the
    /// order is stable, but this type does not sort.
    #[inline]
    pub fn outputs(&self) -> &[OutputFile] {
        &self.outputs
    }

    /// Exclusive directory ownership, including declarations with no files.
    pub fn root_ownerships(&self) -> &[super::owner::OutputRootOwnership] {
        &self.root_ownerships
    }

    /// Counts of HTML pages, other documents, and assets.
    pub fn counts(&self) -> OutputCounts {
        OutputCounts::from_outputs(&self.outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::owner;
    use crate::output::semantics::{self};

    fn output(path: &str, owner: OutputOwner) -> OutputFile {
        let path = OutputPath::parse(path).unwrap();
        let declaration = OutputDeclaration::opaque_from_output_path(&path);
        OutputFile::new(path, declaration, owner, Vec::<u8>::new())
    }

    /// One producer's exclusive claim on a directory root.
    fn root_ownership(path: &str, owner: OutputOwner) -> owner::OutputRootOwnership {
        owner::OutputRootOwnership::new(OutputPath::parse(path).unwrap(), owner)
    }

    #[test]
    fn counts_split_pages_documents_assets() {
        let mut graph = OutputGraphBuilder::new();
        for (path, declaration) in [
            ("index.html", OutputDeclaration::html_document()),
            ("manual.pdf", OutputDeclaration::pdf_document()),
            (
                "styles.css",
                OutputDeclaration::from_filesystem_source(std::path::Path::new("styles.css")),
            ),
        ] {
            let path = OutputPath::parse(path).unwrap();
            graph
                .insert(OutputFile::new(
                    path,
                    declaration,
                    OutputOwner::bundle("site"),
                    Vec::<u8>::new(),
                ))
                .unwrap();
        }

        assert_eq!(
            graph.finish().counts(),
            OutputCounts {
                pages: 1,
                documents: 1,
                assets: 1,
            }
        );
    }

    #[test]
    fn generated_file_media_override_graph() {
        let bytes: Arc<[u8]> = Arc::from(b"<p>Generated</p>".as_slice());
        let generated = GeneratedFile::new(
            "search",
            OutputPath::parse("search/index.html").unwrap(),
            Arc::clone(&bytes),
        )
        .unwrap();
        assert_eq!(generated.producer(), "search");
        assert_eq!(generated.media_type(), &semantics::ResponseMediaType::HTML);
        let generated = generated.with_media_type(semantics::ResponseMediaType::PLAIN_TEXT);
        let mut graph = OutputGraphBuilder::new();
        graph.insert(generated.into_file()).unwrap();
        let graph = graph.finish();
        let output = &graph.outputs()[0];
        assert_eq!(output.kind(), OutputKind::Asset);
        assert_eq!(
            output.declaration().media_type(),
            &semantics::ResponseMediaType::PLAIN_TEXT
        );
        assert_eq!(
            output.declaration().semantics(),
            semantics::DeclaredOutputSemantics::Opaque
        );
        assert!(std::ptr::eq(bytes.as_ptr(), output.bytes().as_ptr()));
    }

    #[test]
    fn blank_or_padded_producers_are_rejected() {
        for producer in ["", " search", "search "] {
            assert!(
                GeneratedFile::new(
                    producer,
                    OutputPath::parse("index.json").unwrap(),
                    b"{}".to_vec()
                )
                .is_err(),
                "{producer:?}"
            );
        }
    }

    #[test]
    fn root_ownership_survives_graph_reuse() {
        let owner = OutputOwner::command(0, "search");
        let ownership = root_ownership("search", owner.clone());
        let mut graph = OutputGraphBuilder::new();
        graph
            .insert(output("search/base.json", OutputOwner::bundle("site")))
            .unwrap();
        assert!(matches!(
            graph.own_root(ownership.clone()),
            Err(OutputGraphError::TreeConflict { .. })
        ));

        let mut generated = OutputGraphBuilder::new();
        generated.own_root(ownership).unwrap();
        let graph = generated.finish();
        let mut reused = OutputGraphBuilder::new();
        reused.insert_graph(&graph).unwrap();
        assert!(matches!(
            reused.insert(output("SEARCH/foreign.json", OutputOwner::system("other"))),
            Err(OutputGraphError::TreeConflict { .. })
        ));
        reused.insert(output("search/index.json", owner)).unwrap();
    }

    #[test]
    fn root_ownership_rejects_overlapping_files() {
        let owner = OutputOwner::command(0, "search");
        let mut graph = OutputGraphBuilder::new();
        graph
            .own_root(root_ownership("search", owner.clone()))
            .unwrap();
        let nested = root_ownership("search/chunks", OutputOwner::command(1, "chunks"));
        assert!(matches!(
            graph.own_root(nested),
            Err(OutputGraphError::TreeConflict { .. })
        ));
        assert!(matches!(
            graph.insert(output("search", owner)),
            Err(OutputGraphError::TreeConflict { .. })
        ));
    }

    #[test]
    fn tree_conflict_names_first_foreign_owner() {
        let first = OutputOwner::command(0, "first");
        let second = OutputOwner::command(1, "second");
        let mut graph = OutputGraphBuilder::new();
        for (path, owner) in [
            ("Straße/z.json", first.clone()),
            ("STRASSE/y.json", first.clone()),
            ("strasse/x.json", second.clone()),
            ("strasse/a.json", second.clone()),
        ] {
            graph.insert(output(path, owner)).unwrap();
        }
        for (owner, expected) in [(first, "strasse/x.json"), (second, "Straße/z.json")] {
            let root = root_ownership("STRASSE", owner);
            let error = graph.own_root(root).unwrap_err();
            assert!(
                matches!(error, OutputGraphError::TreeConflict { path, .. } if path.as_str() == expected)
            );
        }
    }

    /// Spellings that collide on some filesystems are still one output, and the reader learns why
    /// the file and the directory met there.
    #[test]
    fn equivalent_spellings_name_one_output() {
        let mut graph = OutputGraphBuilder::new();
        graph
            .own_root(root_ownership("search", OutputOwner::command(0, "emitter")))
            .unwrap();
        let error = graph
            .insert(output(
                "SEARCH",
                OutputOwner::configured_asset("content/search.json"),
            ))
            .unwrap_err();
        let diagnostic = error_diagnostic(&anyhow::Error::new(error)).unwrap();

        assert_eq!(
            diagnostic.message,
            "`SEARCH` and the directory `search` are the same output"
        );
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("the configured asset `search.json`")),
            "{:?}",
            diagnostic.notes
        );
        assert!(
            diagnostic
                .notes
                .iter()
                .any(|note| note.contains("differ only by letter case or Unicode form")),
            "{:?}",
            diagnostic.notes
        );
    }

    #[test]
    fn output_kind_spelling_is_stable() {
        for (kind, expected) in [
            (OutputKind::HtmlDocument, "html-document"),
            (OutputKind::PdfDocument, "pdf-document"),
            (OutputKind::PngDocument, "png-document"),
            (OutputKind::SvgDocument, "svg-document"),
            (OutputKind::Asset, "asset"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), expected);
            assert_eq!(kind.as_str(), expected);
        }
    }

    /// Two spellings that denote one output path collide, whether a file meets a directory or two
    /// spellings name the same path.
    #[test]
    fn output_path_collisions_are_rejected() {
        for (first, second, expected) in [
            ("docs", "docs/index.html", OutputPathConflict::FileDirectory),
            ("docs/index.html", "docs", OutputPathConflict::FileDirectory),
            (
                "assets/theme",
                "assets/theme/site.css",
                OutputPathConflict::FileDirectory,
            ),
            (
                "Assets",
                "assets/theme.css",
                OutputPathConflict::FileDirectory,
            ),
            (
                "Docs/caf\u{e9}.html",
                "docs/cafe\u{301}.html",
                OutputPathConflict::SamePath,
            ),
            (
                "straße/index.html",
                "STRASSE/index.html",
                OutputPathConflict::SamePath,
            ),
            ("data.json", "data.json", OutputPathConflict::SamePath),
        ] {
            let mut graph = OutputGraphBuilder::new();
            graph
                .insert(output(first, OutputOwner::bundle("site")))
                .unwrap();
            let rejected = graph
                .insert(output(second, OutputOwner::bundle("site")))
                .unwrap_err();
            assert!(
                matches!(
                    rejected,
                    OutputGraphError::PathConflict { conflict, .. } if conflict == expected
                ),
                "{first:?} / {second:?}: {rejected}"
            );
        }
    }

    #[test]
    fn owner_kind_mismatches_are_rejected() {
        for (declaration, owner) in [
            (
                OutputDeclaration::html_document(),
                OutputOwner::configured_asset("content/page.typ"),
            ),
            (
                OutputDeclaration::pdf_document(),
                OutputOwner::system("generated-document"),
            ),
        ] {
            let mut graph = OutputGraphBuilder::new();
            let error = graph
                .insert(OutputFile::new(
                    OutputPath::parse("output.bin").unwrap(),
                    declaration,
                    owner,
                    Vec::<u8>::new(),
                ))
                .unwrap_err();
            assert!(matches!(error, OutputGraphError::OutputOwnerKind { .. }));
        }
    }

    #[test]
    fn output_clones_share_byte_storage() {
        let path = OutputPath::parse("asset.bin").unwrap();
        let declaration = OutputDeclaration::opaque_from_output_path(&path);
        let bytes = b"owned bytes".to_vec();
        let address = bytes.as_ptr();
        let output = OutputFile::from_byte_owner(
            path.clone(),
            declaration.clone(),
            OutputOwner::system("test"),
            bytes,
        );
        assert_eq!(output.bytes().as_ptr(), address);
        let clone = output.clone();
        assert!(output.shares_storage_with(&clone));
        let retained = output.bytes_owner();
        drop(output);
        assert_eq!(retained.as_ref(), b"owned bytes");
        let separate = OutputFile::new(
            path,
            declaration,
            OutputOwner::system("test"),
            b"owned bytes".to_vec(),
        );
        assert!(!clone.shares_storage_with(&separate));
        assert_eq!(clone.digest(), separate.digest());
    }

    #[test]
    fn reserved_namespace_prefixes_are_rejected() {
        let mut graph = OutputGraphBuilder::new();
        assert!(matches!(
            graph
                .insert(output("_TOLA/frame.svg", OutputOwner::bundle("site")))
                .unwrap_err(),
            OutputGraphError::ReservedPath { .. }
        ));
        graph
            .insert(output(
                "_tola/frame.svg",
                OutputOwner::system("svg externalizer"),
            ))
            .unwrap();
    }

    #[test]
    fn duplicate_outputs_name_both_owners() {
        let diagnostic = |conflict, first: &str, second: &str| {
            error_diagnostic(&anyhow::Error::new(OutputGraphError::PathConflict {
                conflict,
                first_path: OutputPath::parse(first).unwrap(),
                first_owner: Box::new(OutputOwner::bundle("site")),
                second_path: OutputPath::parse(second).unwrap(),
                second_owner: Box::new(OutputOwner::configured_asset("content/page.typ")),
            }))
            .expect("a path conflict becomes a diagnostic")
        };

        let identical = diagnostic(
            OutputPathConflict::SamePath,
            "posts/index.html",
            "posts/index.html",
        );
        assert_eq!(identical.code, tola_diagnostic::codes::output::DUPLICATE);
        assert_eq!(identical.message, "`posts/index.html` is published twice");
        assert_eq!(
            identical.notes,
            ["published by the root Bundle and the configured asset `page.typ`"]
        );
        assert_eq!(
            identical.help.first().unwrap().message,
            "give one of them a different path or route"
        );

        let respelled = diagnostic(
            OutputPathConflict::SamePath,
            "Posts/index.html",
            "posts/index.html",
        );
        assert_eq!(
            respelled.message,
            "`Posts/index.html` and `posts/index.html` are the same output"
        );
        assert_eq!(
            respelled.notes[1],
            "the paths differ only by letter case or Unicode form, so they collide on some filesystems"
        );

        let nested = diagnostic(
            OutputPathConflict::FileDirectory,
            "posts.json",
            "posts.json/index.html",
        );
        assert_eq!(
            nested.notes[1],
            "one path is a file and the other is a directory"
        );
    }
}
