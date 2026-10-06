use thiserror::Error;

/// Error during JSON to Typst conversion.
#[derive(Debug, Error)]
pub enum ConvertError {
    /// Required `func` field is missing or is not a string.
    #[error("expected a string field `func` at {path}")]
    MissingFunc {
        /// Slash-separated path of the object.
        path: String,
    },

    /// JSON is not an object.
    #[error("expected JSON object at {path}, got {actual}")]
    ExpectedObject {
        /// Slash-separated path of the value.
        path: String,
        /// Actual JSON type.
        actual: &'static str,
    },

    /// Content that cannot be reconstructed from Typst query JSON.
    #[error("cannot reconstruct Typst `{func}` content from JSON at {path}")]
    Unsupported {
        /// Slash-separated path where the content was found.
        path: String,
        /// Typst content function name.
        func: String,
    },

    /// More than one element function matches the serialized element name.
    #[error("multiple Typst elements match `{func}` at {path}")]
    Ambiguous {
        /// Slash-separated path where the content was found.
        path: String,
        /// Ambiguous serialized function name.
        func: String,
    },

    /// A serialized symbol cannot be represented by one Typst symbol element.
    #[error("invalid symbol at {path}: expected exactly one character")]
    InvalidSymbol {
        /// Slash-separated path where the symbol was found.
        path: String,
    },

    /// Required field is missing or has an invalid type.
    #[error("missing or invalid required field `{field}` at {path}")]
    MissingField {
        /// Slash-separated path of the object.
        path: String,
        /// Required field name.
        field: &'static str,
    },

    /// Function call failed.
    #[error("Typst function `{func}` failed at {path}: {reason}")]
    CallFailed {
        /// Slash-separated path of the content object.
        path: String,
        /// Function name.
        func: String,
        /// Error reason.
        reason: String,
    },

    /// JSON integer cannot be represented by Typst's signed integer type.
    #[error("integer at {path} is out of range for Typst: {value}")]
    IntegerOutOfRange {
        /// Slash-separated path of the number.
        path: String,
        /// Original JSON number.
        value: String,
    },

    /// Invalid literal value for given type.
    #[error("invalid {type_name} literal at {path}: `{value}`")]
    InvalidLiteral {
        /// Slash-separated path of the tagged value.
        path: String,
        /// Expected type name.
        type_name: &'static str,
        /// The invalid value string.
        value: String,
    },

    /// Unknown type tag in `_typst_type` field.
    #[error("unknown `_typst_type` value `{tag}` at {path}")]
    UnknownTypeTag {
        /// Slash-separated path of the tagged value.
        path: String,
        /// Unknown `_typst_type` value.
        tag: String,
    },
}
