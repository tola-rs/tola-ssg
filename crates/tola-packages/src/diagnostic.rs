//! Package argument failures located at the caller rather than the builtin wrapper.

use typst::diag::{SourceDiagnostic, SourceResult};
use typst::ecow::{EcoString, eco_vec};
use typst::foundations::{Str, Value, func};
use typst::syntax::{Span, VirtualRoot};

use crate::TolaPackage;

/// The producer origin of one source metadata issue.
///
/// The diagnostic belongs to the source whose metadata failed, not to a call in the site program,
/// so consumers can classify it without inspecting paths or wording.
pub const SOURCE_ISSUE_ORIGIN: &str = "SourceIssue";

/// The producer origin of a capture the declaration protocol refuses.
///
/// The failure belongs to the channel between a source's scan and the build, not to any call the
/// author wrote, so consumers can report it without inspecting wording.
pub const DECLARATION_PROTOCOL_ORIGIN: &str = "DeclarationProtocol";

pub(crate) fn package_error(
    span: Span,
    package: TolaPackage,
    message: impl Into<EcoString>,
) -> SourceDiagnostic {
    let wrappers = eco_vec![package.spec(), TolaPackage::Host.spec()];
    tola_typst::diagnostic::producer_error(
        span,
        "PackageArgument",
        &[("package", package.name())],
        message,
        tola_typst::diagnostic::DiagnosticProjection::through(wrappers),
    )
}

#[func]
pub(super) fn report_error(
    span: Span,
    /// The package's explanation of the invalid argument or relation.
    message: Str,
    /// A concrete correction supplied by the package.
    #[named]
    #[default]
    hint: Option<Str>,
) -> SourceResult<Value> {
    let package = span.id().and_then(|id| match id.root() {
        VirtualRoot::Package(spec) => TolaPackage::from_spec(spec),
        VirtualRoot::Project => None,
    });
    let mut error = match package {
        Some(package) => package_error(span, package, message.as_str()),
        None => SourceDiagnostic::error(span, message.as_str()),
    };
    if let Some(hint) = hint {
        error.hint(hint.as_str());
    }
    Err(eco_vec![error])
}
