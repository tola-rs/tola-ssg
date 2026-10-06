//! Native metadata declarations and first, all, or unique value selection.

use typst::foundations::{Content, Label, Value};
use typst::introspection::{Location, MetadataElem};
use typst::syntax::Span;
use typst::utils::PicoStr;

#[cfg(feature = "legacy-serialization")]
#[allow(deprecated)]
use crate::codegen::value_to_json;
#[cfg(feature = "legacy-serialization")]
use serde_json::Value as JsonValue;

/// One labeled Typst metadata element from eager evaluation or a compiled Bundle.
///
/// Retains the native value, source span, and optional compiled location.
/// Queries preserve native order without imposing a label or value schema.
#[derive(Debug, Clone)]
pub struct MetadataDeclaration {
    value: Value,
    span: Span,
    location: Option<Location>,
}

impl MetadataDeclaration {
    /// Native value of the metadata element.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Source span of the metadata declaration.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Native location of the realized metadata element, when available.
    ///
    /// Use the Bundle introspector to resolve its containing document or path.
    /// Declarations at the Bundle root have no containing document path.
    pub fn location(&self) -> Option<Location> {
        self.location
    }

    /// Consume the declaration into its native Typst value.
    pub fn into_value(self) -> Value {
        self.value
    }
}

/// More than one metadata declaration matched a unique-value request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`<{label}>` is declared {count} times")]
pub struct MetadataCardinalityError {
    label: String,
    count: usize,
}

impl MetadataCardinalityError {
    /// Label supplied by the caller.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Number of matching metadata declarations.
    pub fn count(&self) -> usize {
        self.count
    }
}

pub(crate) fn unique_metadata_value(
    label: &str,
    values: impl IntoIterator<Item = Value>,
) -> Result<Option<Value>, MetadataCardinalityError> {
    let mut values = values.into_iter();
    let first = values.next();
    let count = usize::from(first.is_some()) + values.count();
    if count > 1 {
        Err(MetadataCardinalityError {
            label: label.to_owned(),
            count,
        })
    } else {
        Ok(first)
    }
}

pub(crate) fn metadata_declaration(content: &Content) -> Option<MetadataDeclaration> {
    let metadata = content.to_packed::<MetadataElem>()?;
    Some(MetadataDeclaration {
        value: metadata.value.clone(),
        span: content.span(),
        location: content.location(),
    })
}

/// Selector matching one metadata label, or `None` when the label is invalid.
pub(crate) fn label_selector(label: &str) -> Option<Label> {
    Label::new(PicoStr::intern(label))
}

/// The first declaration value in native order.
pub(crate) fn first_value(declarations: Vec<MetadataDeclaration>) -> Option<Value> {
    declarations
        .into_iter()
        .next()
        .map(MetadataDeclaration::into_value)
}

/// Every declaration value in native order.
pub(crate) fn all_values(declarations: Vec<MetadataDeclaration>) -> Vec<Value> {
    declarations
        .into_iter()
        .map(MetadataDeclaration::into_value)
        .collect()
}

/// Serialize one optional metadata value at an explicit JSON boundary.
#[cfg(feature = "legacy-serialization")]
#[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
#[allow(deprecated)]
pub(crate) fn json_first(value: Option<Value>) -> serde_json::Result<Option<JsonValue>> {
    value.map(|value| value_to_json(&value)).transpose()
}

/// Serialize every metadata value at an explicit JSON boundary.
#[cfg(feature = "legacy-serialization")]
#[deprecated(note = "native Typst values pass through the pipeline; scheduled for deletion")]
#[allow(deprecated)]
pub(crate) fn json_all(
    values: impl IntoIterator<Item = Value>,
) -> serde_json::Result<Vec<JsonValue>> {
    values
        .into_iter()
        .map(|value| value_to_json(&value))
        .collect()
}

#[cfg(all(test, feature = "scan"))]
mod tests {
    use super::*;

    fn world(source: &str) -> (tempfile::TempDir, crate::TypstWorld) {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("main.typ");
        std::fs::write(&path, source).unwrap();
        let world = crate::TypstWorld::builder(&path, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        (directory, world)
    }

    #[test]
    fn scan_and_compile_agree_on_cardinality() {
        let declarations = "#metadata(1) <record>\n#metadata(2) <record>\n#metadata(3) <single>";
        let (_directory, world) = world(declarations);
        let scanned = crate::scan_world(&world).unwrap();
        let compiled = crate::compile_world(&world).unwrap();
        let html = compiled.document();

        assert_eq!(scanned.metadata_all("record"), html.metadata_all("record"));
        assert_eq!(scanned.metadata_first("record"), Some(Value::Int(1)));
        assert_eq!(html.metadata_first("record"), Some(Value::Int(1)));
        assert_eq!(scanned.metadata_unique("record").unwrap_err().count(), 2);
        assert_eq!(html.metadata_unique("record").unwrap_err().count(), 2);
        assert_eq!(
            scanned.metadata_unique("single").unwrap(),
            Some(Value::Int(3))
        );
        assert_eq!(html.metadata_unique("absent").unwrap(), None);
    }

    #[test]
    fn bundle_document_agrees_on_cardinality() {
        let declarations = "#metadata(1) <record>\n#metadata(2) <record>\n#metadata(3) <single>";
        let (_directory, world) = world(&format!("#document(\"page.html\")[{declarations}]"));
        let bundle =
            crate::compile_bundle_world(&world, &crate::BundleCancellation::new()).unwrap();
        let path = typst::syntax::VirtualPath::new("page.html").unwrap();
        let document = bundle.document(&path).unwrap();

        assert_eq!(
            document.metadata_all("record"),
            [Value::Int(1), Value::Int(2)]
        );
        assert_eq!(document.metadata_first("record"), Some(Value::Int(1)));
        assert_eq!(document.metadata_unique("record").unwrap_err().count(), 2);
        assert_eq!(
            document.metadata_unique("single").unwrap(),
            Some(Value::Int(3))
        );
        assert_eq!(document.metadata_unique("absent").unwrap(), None);
    }
}
