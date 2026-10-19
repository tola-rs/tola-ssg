//! The Tola packages a schema diagnostic points through, not at.

pub(super) const SCHEMA_WRAPPERS: [crate::TolaPackage; 3] = [
    crate::TolaPackage::Schema,
    crate::TolaPackage::Source,
    crate::TolaPackage::Host,
];

/// The packages Tola resolves source metadata through, which no site author wrote: diagnostics
/// reported from inside them point at the author's call and omit their own frames.
pub(super) fn wrapper_projection() -> tola_typst::diagnostic::DiagnosticProjection {
    tola_typst::diagnostic::DiagnosticProjection::through(
        SCHEMA_WRAPPERS.into_iter().map(crate::TolaPackage::spec),
    )
}
