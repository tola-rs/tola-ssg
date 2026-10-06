//! Exported entries: virtual paths, kinds, immutable bytes, and their digests.

use std::sync::Arc;

use typst::model::PagedFormat;
use typst_bundle::{BundleDocument, BundleFile};

/// The kind of an entry emitted by a bundle.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum BundleEntryKind {
    /// An HTML document.
    HtmlDocument,
    /// A PDF document.
    PdfDocument,
    /// A PNG document.
    PngDocument,
    /// An SVG document.
    SvgDocument,
    /// An asset emitted by an `asset` element.
    Asset,
}

/// Export format of a document emitted by a bundle.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BundleDocumentKind {
    /// An HTML document.
    Html,
    /// A PDF document.
    Pdf,
    /// A PNG document.
    Png,
    /// An SVG document.
    Svg,
}

impl BundleEntryKind {
    /// Whether this kind represents a rendered document.
    pub const fn is_document(self) -> bool {
        !matches!(self, Self::Asset)
    }

    /// Whether this kind represents a raw asset.
    pub const fn is_asset(self) -> bool {
        matches!(self, Self::Asset)
    }

    /// Return the document format, or `None` for assets.
    pub const fn document_kind(self) -> Option<BundleDocumentKind> {
        match self {
            Self::HtmlDocument => Some(BundleDocumentKind::Html),
            Self::PdfDocument => Some(BundleDocumentKind::Pdf),
            Self::PngDocument => Some(BundleDocumentKind::Png),
            Self::SvgDocument => Some(BundleDocumentKind::Svg),
            Self::Asset => None,
        }
    }
}

impl BundleDocumentKind {
    /// Return the corresponding Bundle entry kind.
    pub const fn entry_kind(self) -> BundleEntryKind {
        match self {
            Self::Html => BundleEntryKind::HtmlDocument,
            Self::Pdf => BundleEntryKind::PdfDocument,
            Self::Png => BundleEntryKind::PngDocument,
            Self::Svg => BundleEntryKind::SvgDocument,
        }
    }
}

impl From<BundleDocumentKind> for BundleEntryKind {
    fn from(kind: BundleDocumentKind) -> Self {
        kind.entry_kind()
    }
}

/// One immutable in-memory file emitted by a bundle export.
///
/// The digest is computed at construction; all components are immutable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BundleEntry {
    /// The normalized virtual path assigned by Typst.
    pub(super) path: typst::syntax::VirtualPath,
    /// Whether this entry is a document or an asset.
    pub(super) kind: BundleEntryKind,
    /// The exported bytes.
    pub(super) bytes: BundleBytes,
    /// BLAKE3 digest of the exported bytes.
    pub(super) digest: crate::world::file::ContentDigest,
}

impl BundleEntry {
    /// Construct an entry from owned bytes.
    pub fn new(
        path: typst::syntax::VirtualPath,
        kind: BundleEntryKind,
        bytes: impl Into<BundleBytes>,
    ) -> Self {
        let bytes = bytes.into();
        let digest = crate::world::file::ContentDigest::of(bytes.as_slice());
        Self {
            path,
            kind,
            bytes,
            digest,
        }
    }

    /// Return the normalized virtual output path.
    pub fn path(&self) -> &typst::syntax::VirtualPath {
        &self.path
    }

    /// Return whether this entry is a document or an asset.
    pub fn kind(&self) -> BundleEntryKind {
        self.kind
    }

    /// Borrow the immutable exported bytes.
    pub fn bytes(&self) -> &BundleBytes {
        &self.bytes
    }

    /// Return the digest of the exported bytes.
    pub fn digest(&self) -> crate::world::file::ContentDigest {
        self.digest
    }

    /// Consume the entry into its validated components.
    pub fn into_parts(
        self,
    ) -> (
        typst::syntax::VirtualPath,
        BundleEntryKind,
        BundleBytes,
        crate::world::file::ContentDigest,
    ) {
        (self.path, self.kind, self.bytes, self.digest)
    }
}

/// Immutable bytes emitted by Typst's native Bundle exporter.
///
/// Clones share Typst's backing allocation without copying exported bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct BundleBytes(typst::foundations::Bytes);

impl BundleBytes {
    pub(super) fn new(bytes: typst::foundations::Bytes) -> Self {
        Self(bytes)
    }

    /// Access the exported bytes.
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Number of exported bytes.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Whether the export contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Copy the exported bytes into a regular vector.
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl std::fmt::Debug for BundleBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleBytes")
            .field("len", &self.as_slice().len())
            .finish_non_exhaustive()
    }
}

impl AsRef<[u8]> for BundleBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<Vec<u8>> for BundleBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(typst::foundations::Bytes::new(bytes))
    }
}

impl From<typst::foundations::Bytes> for BundleBytes {
    fn from(bytes: typst::foundations::Bytes) -> Self {
        Self::new(bytes)
    }
}

/// Entries retained from one successful native Bundle export.
#[derive(Debug, Clone, Default)]
pub struct BundleEntries(pub(super) Arc<[BundleEntry]>);

impl BundleEntries {
    /// Exported entries in Typst's Bundle order.
    pub fn as_slice(&self) -> &[BundleEntry] {
        &self.0
    }

    /// Number of entries in the export.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the export contains no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over entries in native Bundle order.
    pub fn iter(&self) -> impl Iterator<Item = &BundleEntry> {
        self.0.iter()
    }

    /// Find an entry by its native virtual path.
    pub fn get(&self, path: &typst::syntax::VirtualPath) -> Option<&BundleEntry> {
        self.0.iter().find(|entry| entry.path() == path)
    }
}

impl<'a> IntoIterator for &'a BundleEntries {
    type Item = &'a BundleEntry;
    type IntoIter = std::slice::Iter<'a, BundleEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

pub(crate) fn bundle_file_kind(file: &BundleFile) -> BundleEntryKind {
    match file {
        BundleFile::Document(document) => document_kind(document).into(),
        BundleFile::Asset(_) => BundleEntryKind::Asset,
    }
}

pub(crate) fn document_kind(document: &BundleDocument) -> BundleDocumentKind {
    match document {
        BundleDocument::Html(_) => BundleDocumentKind::Html,
        BundleDocument::Paged(_, extras) => match extras.format {
            PagedFormat::Pdf => BundleDocumentKind::Pdf,
            PagedFormat::Png => BundleDocumentKind::Png,
            PagedFormat::Svg => BundleDocumentKind::Svg,
        },
    }
}
