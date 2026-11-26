//! Package argument diagnostics attributed to the caller rather than builtin wrappers.

use typst::diag::{SourceDiagnostic, SourceResult};
use typst::ecow::{EcoString, eco_vec};
use typst::foundations::{Str, Value, func};
use typst::syntax::Span;

use crate::TolaPackage;

/// Producer origin name for source metadata issues.
pub const SOURCE_ISSUE_ORIGIN: &str = "SourceIssue";

/// Producer origin name for refused metadata capture channels.
pub const DECLARATION_PROTOCOL_ORIGIN: &str = "DeclarationProtocol";

pub(crate) fn package_error(
    span: Span,
    pkg: TolaPackage,
    msg: impl Into<EcoString>,
) -> SourceDiagnostic {
    let wrappers = eco_vec![pkg.spec(), TolaPackage::Host.spec()];
    tola_typst::diagnostic::producer_error(
        span,
        "PackageArgument",
        &[("package", pkg.name())],
        msg,
        tola_typst::diagnostic::DiagnosticProjection::through(wrappers),
    )
}

#[func]
pub(super) fn report_error(
    span: Span,
    /// Explanation of the invalid argument or relation.
    message: Str,
    /// Concrete correction supplied by the package.
    #[named]
    #[default]
    hint: Option<Str>,
) -> SourceResult<Value> {
    let mut diag = match span.id().and_then(crate::pkg_of_file) {
        Some(pkg) => package_error(span, pkg, message.as_str()),
        None => SourceDiagnostic::error(span, message.as_str()),
    };
    if let Some(hint) = hint {
        diag.hint(hint.as_str());
    }
    Err(eco_vec![diag])
}
