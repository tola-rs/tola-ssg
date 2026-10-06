//! HTML document wrapper.

use typst::foundations::{Selector, Value};
use typst::introspection::Introspector;

use crate::introspection::{
    MetadataCardinalityError, MetadataDeclaration, all_values, first_value, label_selector,
    metadata_declaration, unique_metadata_value,
};
#[cfg(feature = "legacy-serialization")]
#[allow(deprecated)]
use crate::introspection::{json_all, json_first};

/// A compiled HTML document.
///
/// Adds narrow metadata helpers without mirroring Typst's public DOM types.
#[derive(Debug, Clone)]
pub struct HtmlDocument(pub(crate) typst_html::HtmlDocument);

impl HtmlDocument {
    /// Wrap a native Typst HTML document.
    #[inline]
    pub fn new(doc: typst_html::HtmlDocument) -> Self {
        Self(doc)
    }

    /// Typst's complete introspection of this document.
    #[inline]
    pub fn introspector(&self) -> &dyn Introspector {
        self.0.introspector().as_ref()
    }

    /// Root element of the document.
    #[inline]
    pub fn root(&self) -> &typst_html::HtmlElement {
        self.0.root()
    }

    /// Inspect the final Typst DOM without serializing or reparsing HTML bytes.
    pub fn inventory(
        &self,
        cancellation: &crate::BundleCancellation,
    ) -> Result<super::HtmlDocumentInventory, crate::diagnostic::CompileError> {
        super::HtmlDocumentInventory::from_document(&self.0, cancellation)
    }

    /// First metadata value matching the label in native document order.
    pub fn metadata_first(&self, label: &str) -> Option<Value> {
        first_value(self.metadata_declarations(label))
    }

    /// One metadata value, distinguishing absence from duplicate declarations.
    pub fn metadata_unique(&self, label: &str) -> Result<Option<Value>, MetadataCardinalityError> {
        unique_metadata_value(label, self.metadata_all(label))
    }

    /// Every matching native metadata value in document order.
    pub fn metadata_all(&self, label: &str) -> Vec<Value> {
        all_values(self.metadata_declarations(label))
    }

    /// Native declarations with their source spans and compiled locations.
    pub fn metadata_declarations(&self, label: &str) -> Vec<MetadataDeclaration> {
        let Some(label) = label_selector(label) else {
            return Vec::new();
        };
        self.0
            .introspector()
            .query(&Selector::Label(label))
            .iter()
            .filter_map(metadata_declaration)
            .collect()
    }

    /// Serialize the first matching value at an explicit JSON boundary.
    #[cfg(feature = "legacy-serialization")]
    #[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
    #[allow(deprecated)]
    pub fn metadata_json_first(
        &self,
        label: &str,
    ) -> serde_json::Result<Option<serde_json::Value>> {
        json_first(self.metadata_first(label))
    }

    /// Serialize every matching value at an explicit JSON boundary.
    #[cfg(feature = "legacy-serialization")]
    #[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
    #[allow(deprecated)]
    pub fn metadata_json_all(&self, label: &str) -> serde_json::Result<Vec<serde_json::Value>> {
        json_all(self.metadata_all(label))
    }

    /// Unwrap the native Typst HTML document.
    #[inline]
    pub fn into_inner(self) -> typst_html::HtmlDocument {
        self.0
    }

    /// Borrow the native Typst HTML document.
    #[inline]
    pub fn as_inner(&self) -> &typst_html::HtmlDocument {
        &self.0
    }
}

impl From<typst_html::HtmlDocument> for HtmlDocument {
    fn from(doc: typst_html::HtmlDocument) -> Self {
        Self::new(doc)
    }
}
