//! Reference diagnostics applied to a sealed site build.

use tola_address::OutputPath;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::config::{ReferenceLevel, ResolvedSiteConfig};
use crate::diagnostic::{Diagnostic, DiagnosticCause, Severity};
use crate::site::references::{
    Reference, ReferenceCategory, ReferenceResolution, References, UnresolvedReferenceReason,
};

// Resolution groups may be reused, but policy is always projected from this
// build's configuration so cached failures cannot retain an obsolete severity.
pub(crate) fn diagnostics(
    index: &References,
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<Vec<Diagnostic>, BuildCancelled> {
    cancellation.ensure_active()?;
    let mut diagnostics = Vec::new();
    for reference in index.references() {
        cancellation.ensure_active()?;
        if let Some(diagnostic) = reference_diagnostic(config, reference) {
            let identity = crate::diagnostic::ReferenceIdentity::of(&diagnostic);
            diagnostics.push((identity, diagnostic, reference.page().as_str().to_owned()));
        }
    }
    for (page, base) in index.unchecked_bases() {
        cancellation.ensure_active()?;
        let diagnostic = base_href_diagnostic(page, base);
        let identity = crate::diagnostic::ReferenceIdentity::of(&diagnostic);
        diagnostics.push((identity, diagnostic, page.as_str().to_owned()));
    }
    cancellation.ensure_active()?;
    Ok(crate::diagnostic::collapse_repeated(diagnostics))
}

/// The page whose document base names another origin, reported rather than silently unchecked.
///
/// Nothing is wrong with the page's published content; it is the checks that do not reach it, so
/// this is always a warning rather than a failure.
fn base_href_diagnostic(
    page: &OutputPath,
    base: &crate::site::references::UncheckedBase,
) -> Diagnostic {
    let message = format!(
        "`{}` names a host, so this page's relative references are not checked",
        base.value
    );
    match &base.origin {
        Some(origin) => Diagnostic::at_location(
            crate::codes::reference::BASE_HREF_EXTERNAL_ORIGIN,
            Severity::Warning,
            origin.clone(),
            message,
        ),
        None => Diagnostic::at_path(
            crate::codes::reference::BASE_HREF_EXTERNAL_ORIGIN,
            Severity::Warning,
            page.as_str(),
            message,
        ),
    }
    .with_help("remove the `<base href>` or write it as a relative address (`/`, `/docs/`)")
}
fn reference_diagnostic(config: &ResolvedSiteConfig, reference: &Reference) -> Option<Diagnostic> {
    let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
        return None;
    };
    let (default_code, level) = match reference.category() {
        ReferenceCategory::Navigation => (
            crate::codes::reference::NAVIGATION_MISSING,
            config.build.references.navigation,
        ),
        ReferenceCategory::Resource => (
            crate::codes::reference::RESOURCE_MISSING,
            config.build.references.resources,
        ),
        ReferenceCategory::Fragment => (
            crate::codes::reference::FRAGMENT_MISSING,
            config.build.references.fragments,
        ),
        ReferenceCategory::External => return None,
    };
    let code = if reason.is_media_type_mismatch() {
        crate::codes::reference::RESOURCE_MEDIA_MISMATCH
    } else {
        default_code
    };
    let severity = match level {
        ReferenceLevel::Error => Severity::Error,
        ReferenceLevel::Warn => Severity::Warning,
    };
    let message = format!(
        "`{}` {}",
        reference.destination(),
        reason.message(reference.category())
    );
    let mut diagnostic = match reference.origin() {
        Some(origin) => Diagnostic::at_location(code, severity, origin.clone(), message),
        None => Diagnostic::at_path(code, severity, reference.page().as_str(), message)
            .with_note(reference.html_context()),
    };
    if let Some(help) = reason.help(reference.category()) {
        diagnostic = diagnostic.with_help(help);
    }
    if let Some(cause) = reference_correction(reference) {
        diagnostic = diagnostic.with_cause(cause);
    }
    Some(diagnostic)
}

/// The destinations that would resolve one reference, when the site can name them.
///
/// A correction names whole destinations, so the edit that applies one replaces the destination
/// the source wrote without reading the rendered message or searching the document for a
/// spelling. Every destination is written the way a browser writes it: the site's address layer
/// owns percent-encoding and the mount prefix, so no editor reassembles a URL.
fn reference_correction(reference: &Reference) -> Option<DiagnosticCause> {
    let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
        return None;
    };
    let destination = reference.destination();
    let replacements = match reason {
        // The author wrote a directory page's address without the slash its route has.
        UnresolvedReferenceReason::TargetMissing {
            suggestion: Some(address),
        } => vec![address.clone()],
        // A fragment names an id the target document does not have, and every id it does have
        // would resolve: the author chooses among the ones the diagnostic's own help lists.
        UnresolvedReferenceReason::FragmentMissing { available, .. } => {
            fragment_replacements(destination, available)
        }
        _ => return None,
    };
    // A destination the source already writes resolves nothing, so offering it would offer no
    // change at all.
    let replacements: Vec<String> = replacements
        .into_iter()
        .filter(|replacement| replacement != destination)
        .collect();
    (!replacements.is_empty()).then(|| DiagnosticCause::UnresolvedReference {
        destination: destination.to_owned(),
        replacements,
    })
}

/// The destinations that keep everything the source wrote before the fragment and name one id
/// the target document offers.
///
/// An id a browser URL cannot hold as a fragment — one holding a separator or a control
/// character — offers no destination.
fn fragment_replacements(destination: &str, available: &[String]) -> Vec<String> {
    let Some(fragment) = destination.find('#') else {
        return Vec::new();
    };
    let before = &destination[..=fragment];
    available
        .iter()
        .take(crate::diagnostic::LISTED_NAMES)
        .map(|id| tola_address::browser_location(id))
        .filter(|id| !id.contains(['?', '#', '"', '\\']) && !id.chars().any(char::is_control))
        .map(|id| format!("{before}{id}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::references::{Reference, UnresolvedReferenceReason};

    fn unresolved(
        destination: &str,
        category: ReferenceCategory,
        reason: UnresolvedReferenceReason,
    ) -> Reference {
        Reference {
            page: tola_address::OutputPath::parse("index.html").unwrap(),
            destination: destination.to_owned(),
            html_context: "a[href]".to_owned(),
            category,
            resolution: ReferenceResolution::Unresolved { reason },
            origin: None,
        }
    }

    fn cause(reference: &Reference) -> Option<DiagnosticCause> {
        let directory = tempfile::TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        reference_diagnostic(&config, reference).and_then(|diagnostic| diagnostic.cause)
    }

    #[test]
    fn missing_directory_page_offers_its_published_address() {
        let reference = unresolved(
            "/post",
            ReferenceCategory::Navigation,
            UnresolvedReferenceReason::TargetMissing {
                suggestion: Some("/post/".to_owned()),
            },
        );

        assert_eq!(
            cause(&reference),
            Some(DiagnosticCause::UnresolvedReference {
                destination: "/post".to_owned(),
                replacements: vec!["/post/".to_owned()],
            })
        );
    }

    #[test]
    fn missing_fragment_offers_each_id_the_target_has() {
        let reference = unresolved(
            "/post/#missing",
            ReferenceCategory::Fragment,
            UnresolvedReferenceReason::FragmentMissing {
                fragment: "missing".to_owned(),
                available: ["a".to_owned(), "z".to_owned()].into(),
            },
        );

        assert_eq!(
            cause(&reference),
            Some(DiagnosticCause::UnresolvedReference {
                destination: "/post/#missing".to_owned(),
                replacements: vec!["/post/#a".to_owned(), "/post/#z".to_owned()],
            })
        );
    }

    #[test]
    fn missing_target_with_no_address_offers_nothing() {
        let reference = unresolved(
            "/nowhere.html",
            ReferenceCategory::Navigation,
            UnresolvedReferenceReason::TargetMissing { suggestion: None },
        );

        assert_eq!(cause(&reference), None);
    }
}
