//! Reference diagnostics applied to a sealed site build.

use tola_address::OutputPath;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::config::{ReferenceLevel, ResolvedSiteConfig};
use crate::site::references::{
    Reference, ReferenceCategory, ReferenceResolution, References, UnresolvedReferenceReason,
};
use tola_diagnostic::{Diagnostic, DiagnosticCause, Severity};

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
            let identity = tola_diagnostic::ReferenceIdentity::of(&diagnostic);
            diagnostics.push((identity, diagnostic, reference.page().as_str().to_owned()));
        }
    }
    for (page, base) in index.unchecked_bases() {
        cancellation.ensure_active()?;
        let diagnostic = base_href_diagnostic(page, base);
        let identity = tola_diagnostic::ReferenceIdentity::of(&diagnostic);
        diagnostics.push((identity, diagnostic, page.as_str().to_owned()));
    }
    cancellation.ensure_active()?;
    Ok(tola_diagnostic::collapse_repeated(diagnostics))
}

/// The page whose document base names another origin, reported rather than silently unchecked.
///
/// Nothing is wrong with the page's published content; it is the checks that do not reach it, so
/// this is always a warning rather than a failure.
fn base_href_diagnostic(page: &OutputPath, base: &super::document::UncheckedBase) -> Diagnostic {
    let message = format!(
        "`{}` names a host, so this page's relative references are not checked",
        base.value
    );
    match &base.origin {
        Some(origin) => Diagnostic::at_location(
            tola_diagnostic::codes::reference::BASE_HREF_EXTERNAL_ORIGIN,
            Severity::Warning,
            origin.clone(),
            message,
        ),
        None => Diagnostic::at_path(
            tola_diagnostic::codes::reference::BASE_HREF_EXTERNAL_ORIGIN,
            Severity::Warning,
            page.as_str(),
            message,
        ),
    }
    .with_help("Remove the `<base href>` or write it as a relative address (`/`, `/docs/`)")
}

fn reference_diagnostic(config: &ResolvedSiteConfig, reference: &Reference) -> Option<Diagnostic> {
    let ReferenceResolution::Unresolved { reason } = reference.resolution() else {
        return None;
    };
    let (default_code, level) = match reference.category() {
        ReferenceCategory::Navigation => (
            tola_diagnostic::codes::reference::NAVIGATION_MISSING,
            config.build.references.navigation,
        ),
        ReferenceCategory::Resource => (
            tola_diagnostic::codes::reference::RESOURCE_MISSING,
            config.build.references.resources,
        ),
        ReferenceCategory::Fragment => (
            tola_diagnostic::codes::reference::FRAGMENT_MISSING,
            config.build.references.fragments,
        ),
        ReferenceCategory::External => return None,
    };
    let code = if matches!(reason, UnresolvedReferenceReason::MediaTypeMismatch { .. }) {
        tola_diagnostic::codes::reference::RESOURCE_MEDIA_MISMATCH
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
    if let Some(cause) = reference_correction(reference.destination(), reason) {
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
fn reference_correction(
    destination: &str,
    reason: &UnresolvedReferenceReason,
) -> Option<DiagnosticCause> {
    let mut replacements = match reason {
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
    replacements.retain(|replacement| replacement != destination);
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
        .take(tola_diagnostic::LISTED_NAMES)
        .map(|id| tola_address::browser_location(id))
        .filter(|id| !id.contains(['?', '#', '"', '\\']) && !id.chars().any(char::is_control))
        .map(|id| format!("{before}{id}"))
        .collect()
}

impl UnresolvedReferenceReason {
    /// Clause that continues the destination named by the rendered diagnostic.
    pub fn message(&self, category: ReferenceCategory) -> String {
        match self {
            Self::OutsideSiteMount {
                required_prefix, ..
            } => {
                format!(
                    "is outside `site.base-path`, so the site is served under `{required_prefix}`"
                )
            }
            Self::InvalidBrowserReference { message } => message.clone(),
            Self::TargetMissing { .. } => match category {
                ReferenceCategory::Resource => "is not a file this site publishes".into(),
                _ => "is not a page this site publishes".into(),
            },
            Self::FragmentMissing { fragment, .. } => {
                format!("has no element with the id `{fragment}`")
            }
            Self::DocumentNotAllowed { .. } => {
                "is an HTML page, but this attribute needs a file".to_string()
            }
            Self::MediaTypeMismatch {
                reference_use,
                required,
                declared,
            } => format!(
                "is used as {}, which needs `{}`, but the file is served as `{}`",
                super::target::expectation(*reference_use).usage,
                required.as_str(),
                declared.as_str()
            ),
        }
    }

    pub fn help(&self, category: ReferenceCategory) -> Option<String> {
        match self {
            Self::OutsideSiteMount {
                required_prefix, ..
            } => Some(format!(
                "Start root-relative URLs with `{required_prefix}` (from `site.base-path`) or make the link relative"
            )),
            Self::TargetMissing {
                suggestion: Some(suggestion),
            } => Some(format!("Point the link at `{suggestion}`")),
            Self::TargetMissing { suggestion: None } => Some(match category {
                ReferenceCategory::Resource => {
                    "Declare it in `assets.files` or `assets.trees` in `tola.toml`, or take its URL from `asset-url()`".into()
                }
                _ => {
                    "Point the link at a page that exists, or create the page that publishes it"
                        .into()
                }
            }),
            Self::FragmentMissing { available, .. } if !available.is_empty() => Some(format!(
                "Choose an existing id: {}",
                tola_diagnostic::bounded_listing(available, ("id", "ids"))
            )),
            Self::DocumentNotAllowed { .. } => {
                Some("Point it at a generated file such as a stylesheet, script, or image".into())
            }
            Self::MediaTypeMismatch { .. } => Some(
                "Give the file the extension this attribute needs, or use the matching attribute"
                    .into(),
            ),
            Self::InvalidBrowserReference { .. } | Self::FragmentMissing { .. } => None,
        }
    }
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
    fn missing_directory_page_published_address() {
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
    fn missing_each_id_target_has() {
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
}
