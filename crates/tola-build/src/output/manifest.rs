//! Output representations used to identify and compare complete site revisions.

use serde::{Serialize, Serializer};

use super::graph::{OutputFile, OutputGraph, OutputKind};
use super::semantics::{DeclaredOutputSemantics, ResponseMediaType};
use super::summary::OutputCounts;
use tola_address::OutputPath;
use tola_typst::hash_length_prefixed;

/// Stable digest identity of a complete ordered output manifest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RevisionId(String);

/// Identity of one output's path, bytes, document kind, and response semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepresentationId([u8; 32]);

impl RevisionId {
    fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl RepresentationId {
    pub fn from_output(output: &OutputFile) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"tola-output-representation\0");
        hash_length_prefixed(&mut hasher, output.path().as_str().as_bytes());
        hash_length_prefixed(&mut hasher, output.kind().as_str().as_bytes());
        hash_length_prefixed(
            &mut hasher,
            output.declaration().semantics().as_str().as_bytes(),
        );
        hash_length_prefixed(
            &mut hasher,
            output.declaration().media_type().as_str().as_bytes(),
        );
        hasher.update(output.digest().as_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    fn matches_hex(&self, value: &str) -> bool {
        blake3::Hash::from(self.0).to_hex().as_str() == value
    }

    pub fn to_hex(self) -> String {
        blake3::Hash::from(self.0).to_hex().to_string()
    }
}

impl Serialize for RepresentationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(blake3::Hash::from(self.0).to_hex().as_str())
    }
}

/// Manifest for one complete validated output graph.
///
/// Constructing a manifest neither writes outputs nor publishes a revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteManifest {
    revision: RevisionId,
    outputs: Vec<ManifestEntry>,
}

/// Changes between two complete output revisions, ordered by logical path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionDiff {
    from: RevisionId,
    to: RevisionId,
    changes: Vec<OutputDelta>,
}

/// Addition, removal, or replacement of one output representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "operation", content = "output", rename_all = "lowercase")]
pub enum OutputDelta {
    Added(ManifestEntry),
    Removed(ManifestEntry),
    Modified {
        before: ManifestEntry,
        after: ManifestEntry,
    },
}

/// One output's immutable identity and declared delivery metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestEntry {
    path: OutputPath,
    representation: RepresentationId,
    size: u64,
    kind: OutputKind,
    semantics: DeclaredOutputSemantics,
    media_type: ResponseMediaType,
}

impl SiteManifest {
    /// Describe a complete graph in deterministic logical output order.
    pub fn from_graph(graph: &OutputGraph) -> Self {
        let mut outputs = graph
            .outputs()
            .iter()
            .map(|output| ManifestEntry {
                path: output.path().clone(),
                kind: output.kind(),
                semantics: output.declaration().semantics(),
                media_type: output.declaration().media_type().clone(),
                size: output.bytes().len() as u64,
                representation: RepresentationId::from_output(output),
            })
            .collect::<Vec<_>>();
        outputs.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        let revision = RevisionId::new(revision_digest(&outputs));

        Self { revision, outputs }
    }

    #[inline]
    pub fn revision(&self) -> &RevisionId {
        &self.revision
    }

    /// Outputs in lexicographic path order.
    pub fn outputs(&self) -> &[ManifestEntry] {
        &self.outputs
    }

    pub fn diff(&self, next: &Self) -> RevisionDiff {
        let mut changes = Vec::new();
        let mut before = self.outputs.iter().peekable();
        let mut after = next.outputs.iter().peekable();

        loop {
            match (before.peek(), after.peek()) {
                (Some(left), Some(right)) => match left.path.cmp(&right.path) {
                    std::cmp::Ordering::Less => {
                        changes.push(OutputDelta::Removed((*left).clone()));
                        before.next();
                    }
                    std::cmp::Ordering::Greater => {
                        changes.push(OutputDelta::Added((*right).clone()));
                        after.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if left.representation != right.representation {
                            changes.push(OutputDelta::Modified {
                                before: (*left).clone(),
                                after: (*right).clone(),
                            });
                        }
                        before.next();
                        after.next();
                    }
                },
                (Some(left), None) => {
                    changes.push(OutputDelta::Removed((*left).clone()));
                    before.next();
                }
                (None, Some(right)) => {
                    changes.push(OutputDelta::Added((*right).clone()));
                    after.next();
                }
                (None, None) => break,
            }
        }

        RevisionDiff {
            from: self.revision.clone(),
            to: next.revision.clone(),
            changes,
        }
    }

    pub fn matches_representation(&self, path: &OutputPath, representation: &str) -> bool {
        self.manifest_entry(path)
            .is_some_and(|output| output.representation.matches_hex(representation))
    }

    fn manifest_entry(&self, path: &OutputPath) -> Option<&ManifestEntry> {
        self.outputs
            .binary_search_by(|output| output.path.cmp(path))
            .ok()
            .map(|index| &self.outputs[index])
    }
}

impl RevisionDiff {
    pub fn from(&self) -> &RevisionId {
        &self.from
    }

    pub fn to(&self) -> &RevisionId {
        &self.to
    }

    pub fn changes(&self) -> &[OutputDelta] {
        &self.changes
    }

    pub fn is_unchanged(&self) -> bool {
        self.from == self.to && self.changes.is_empty()
    }

    pub fn changed_output_counts(&self) -> OutputCounts {
        let mut counts = OutputCounts::default();
        for change in &self.changes {
            let kind = match change {
                OutputDelta::Added(output) | OutputDelta::Removed(output) => output.kind,
                OutputDelta::Modified { after, .. } => after.kind,
            };
            counts.add_kind(kind);
        }
        counts
    }
}

impl ManifestEntry {
    pub fn path(&self) -> &OutputPath {
        &self.path
    }

    pub fn representation(&self) -> RepresentationId {
        self.representation
    }

    /// Number of uncompressed output bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn kind(&self) -> OutputKind {
        self.kind
    }

    pub fn semantics(&self) -> DeclaredOutputSemantics {
        self.semantics
    }

    pub fn media_type(&self) -> &ResponseMediaType {
        &self.media_type
    }
}

fn revision_digest<'a>(outputs: impl IntoIterator<Item = &'a ManifestEntry>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"tola-site-manifest\0");
    for output in outputs {
        hasher.update(&output.representation.0);
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::graph::{OutputFile, OutputGraphBuilder};
    use crate::output::semantics::OutputDeclaration;
    use crate::output::tests::{output_file, output_graph};

    fn asset_output(path: &str, bytes: &[u8]) -> OutputFile {
        let output_path = OutputPath::parse(path).unwrap();
        output_file(
            path,
            OutputDeclaration::opaque_from_output_path(&output_path),
            bytes,
        )
    }

    fn html_output(path: &str, bytes: &[u8]) -> OutputFile {
        output_file(path, OutputDeclaration::html_document(), bytes)
    }

    #[test]
    fn manifest_ignores_insertion_order() {
        let first = SiteManifest::from_graph(&output_graph([
            asset_output("b.json", b"B"),
            html_output("a.html", b"A"),
        ]));
        let second = SiteManifest::from_graph(&output_graph([
            html_output("a.html", b"A"),
            asset_output("b.json", b"B"),
        ]));

        assert_eq!(first, second);
        assert_eq!(
            first
                .outputs()
                .iter()
                .map(|output| output.path().as_str())
                .collect::<Vec<_>>(),
            ["a.html", "b.json"]
        );
    }

    #[test]
    fn every_output_change_moves_the_revision() {
        let baseline = SiteManifest::from_graph(&output_graph([html_output("index.html", b"one")]));
        for changed in [
            html_output("index.html", b"two"),
            html_output("other.html", b"one"),
            asset_output("index.html", b"one"),
        ] {
            let changed = SiteManifest::from_graph(&output_graph([changed]));
            assert_ne!(baseline.revision(), changed.revision());
        }
    }

    #[test]
    fn declaration_change_reports_modification() {
        let before = SiteManifest::from_graph(&output_graph([output_file(
            "styles/app",
            OutputDeclaration::opaque(ResponseMediaType::OCTET_STREAM),
            b"body {}",
        )]));
        let after = SiteManifest::from_graph(&output_graph([output_file(
            "styles/app",
            OutputDeclaration::from_filesystem_source(std::path::Path::new("styles/app.css")),
            b"body {}",
        )]));

        assert_ne!(
            before.outputs()[0].representation(),
            after.outputs()[0].representation()
        );
        assert_ne!(before.revision(), after.revision());
        assert!(matches!(
            before.diff(&after).changes(),
            [OutputDelta::Modified { before, after }]
                if before.semantics != after.semantics
                    && before.media_type != after.media_type
        ));

        let encoded = serde_json::to_value(before.diff(&after)).unwrap();
        assert_eq!(
            encoded["changes"][0]["output"]["after"]["media_type"],
            ResponseMediaType::CSS.as_str()
        );
    }

    #[test]
    fn diff_lists_changes_in_path_order() {
        let before = SiteManifest::from_graph(&output_graph([
            asset_output("a.txt", b"removed"),
            asset_output("b.txt", b"before"),
            asset_output("c.txt", b"stable"),
        ]));
        let after = SiteManifest::from_graph(&output_graph([
            asset_output("b.txt", b"after"),
            asset_output("c.txt", b"stable"),
            asset_output("d.txt", b"added"),
        ]));

        let diff = before.diff(&after);

        assert_eq!(&diff.from, before.revision());
        assert_eq!(&diff.to, after.revision());
        assert_eq!(diff.changes().len(), 3);
        assert_eq!(
            diff.changed_output_counts(),
            OutputCounts {
                pages: 0,
                documents: 0,
                assets: 3,
            }
        );
        assert!(matches!(
            &diff.changes()[0],
            OutputDelta::Removed(output) if output.path.as_str() == "a.txt"
        ));
        assert!(matches!(
            &diff.changes()[1],
            OutputDelta::Modified { before, after }
                if before.path.as_str() == "b.txt"
                    && after.path.as_str() == "b.txt"
                    && before.representation != after.representation
        ));
        assert!(matches!(
            &diff.changes()[2],
            OutputDelta::Added(output) if output.path.as_str() == "d.txt"
        ));
    }

    #[test]
    fn identical_output_sets_report_no_change() {
        let same = SiteManifest::from_graph(&output_graph([html_output("index.html", b"same")]));

        let identical = same.diff(&same);
        assert!(identical.changes().is_empty());
        assert_eq!(identical.from(), identical.to());
        assert!(identical.is_unchanged());

        let before =
            SiteManifest::from_graph(&output_graph([html_output("index.html", b"before")]));
        let after = SiteManifest::from_graph(&output_graph([html_output("index.html", b"after")]));
        assert!(!before.diff(&after).is_unchanged());
    }

    #[test]
    fn representation_lookup_needs_exact_hex() {
        let manifest =
            SiteManifest::from_graph(&output_graph([asset_output("styles/site.css", b"body {}")]));
        let path = OutputPath::parse("styles/site.css").unwrap();
        let representation = manifest.outputs()[0].representation().to_hex();

        assert!(manifest.matches_representation(&path, &representation));
        assert!(!manifest.matches_representation(&path, "stale"));
        assert!(!manifest.matches_representation(
            &OutputPath::parse("styles/other.css").unwrap(),
            &representation,
        ));
    }

    #[test]
    fn diff_serializes_representation_as_hex() {
        let before = SiteManifest::from_graph(&OutputGraphBuilder::new().finish());
        let after =
            SiteManifest::from_graph(&output_graph([asset_output("styles/site.css", b"body {}")]));

        let encoded = serde_json::to_value(before.diff(&after)).unwrap();

        assert_eq!(
            encoded["changes"][0]["output"]["representation"],
            after.outputs()[0].representation().to_hex()
        );
    }

    #[test]
    fn kind_change_reports_modification() {
        let before = SiteManifest::from_graph(&output_graph([asset_output("result", b"same")]));
        let after = SiteManifest::from_graph(&output_graph([html_output("result", b"same")]));

        assert!(matches!(
            after.diff(&before).changes(),
            [OutputDelta::Modified { before, after }]
                if before.kind == OutputKind::HtmlDocument
                    && after.kind == OutputKind::Asset
                    && before.size == after.size
        ));
    }
}
