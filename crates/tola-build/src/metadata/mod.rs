//! Source declarations retained across eager analysis and root Bundle evaluation.

use std::sync::Arc;

use typst::foundations::Dict;
use typst::utils::LazyHash;

/// The label the retired declaration spelling writes.
///
/// Nothing registers through it any more: a source's declaration is the native event analysis
/// decodes from its own scan. Analysis reads this label only to recognize — and warn about —
/// sources still written as `#metadata((...)) <tola-meta>`.
pub const SOURCE_METADATA_LABEL: &str = "tola-meta";

/// One physical source's metadata declaration.
///
/// Native values retain their types, styles, closures, and deferred content.
/// Equality follows Typst's memoization semantics without rendering or serialization.
/// The dictionary is evaluated eagerly; final document queries use the compiled Bundle.
///
/// One eager `tola-meta(...)` call declares for the source evaluating it, including through
/// imported functions. Imported or included modules retain their own declarations. Compiled
/// metadata queries are unrestricted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMetadata {
    fields: Arc<LazyHash<Dict>>,
}

impl SourceMetadata {
    /// Wrap the dictionary one native declaration held.
    pub fn from_dict(fields: Dict) -> Self {
        Self {
            fields: Arc::new(LazyHash::new(fields)),
        }
    }

    pub fn to_typst_dict(&self) -> Dict {
        (**self.fields).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Array, IntoValue, Label, Value};
    use typst::text::TextElem;

    /// The dictionary one declaration has, with `field` set to `value`.
    fn declared(field: &str, value: Value) -> Dict {
        let mut fields = Dict::new();
        fields.insert(field.into(), value);
        fields
    }

    #[test]
    fn metadata_identity_distinguishes_values() {
        let integer = Array::from_iter([Value::Int(1)]).into_value();
        let float = Array::from_iter([Value::Float(1.0)]).into_value();
        let mut first_then_second = Dict::new();
        first_then_second.insert("first".into(), Value::Int(1));
        first_then_second.insert("second".into(), Value::Int(2));
        let mut second_then_first = Dict::new();
        second_then_first.insert("second".into(), Value::Int(2));
        second_then_first.insert("first".into(), Value::Int(1));
        let labelled = |label: &str| {
            TextElem::packed("Document")
                .labelled(Label::construct(label.into()).unwrap())
                .into_value()
        };

        for (field, left, right) in [
            ("ranks", integer, float),
            ("offset", Value::Float(-0.0), Value::Float(0.0)),
            (
                "navigation",
                first_then_second.into_value(),
                second_then_first.into_value(),
            ),
            ("card", labelled("first"), labelled("second")),
        ] {
            let left = SourceMetadata::from_dict(declared(field, left));
            assert_ne!(
                left,
                SourceMetadata::from_dict(declared(field, right)),
                "{field}"
            );
            assert_eq!(left, left.clone(), "{field}");
        }
    }
}
