//! Deterministic UUID operations exposed to Typst.

use typst::diag::{SourceResult, bail};
use typst::foundations::{Str, func};
use typst::loading::Readable;
use typst::syntax::Span;
use uuid::Uuid;

/// Normalize a UUID's textual spelling.
#[func]
pub(super) fn normalize(
    span: Span,
    /// UUID string to normalize.
    value: Str,
) -> SourceResult<Str> {
    let Ok(id) = Uuid::parse_str(value.as_str()) else {
        bail!(span, "invalid UUID: expected a UUID string");
    };
    Ok(id.hyphenated().to_string().into())
}

/// Calculate a name-based version 5 UUID.
#[func]
pub(super) fn v5(
    span: Span,
    /// Namespace UUID.
    namespace: Str,
    /// Name whose bytes determine the UUID.
    name: Readable,
) -> SourceResult<Str> {
    let Ok(namespace) = Uuid::parse_str(namespace.as_str()) else {
        bail!(span, "invalid UUID namespace: expected a UUID string");
    };
    let bytes = name.into_bytes();
    Ok(Uuid::new_v5(&namespace, bytes.as_slice())
        .hyphenated()
        .to_string()
        .into())
}
