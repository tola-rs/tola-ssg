//! Diagnostics attached at complete build and output-write boundaries.

use std::path::Path;

use tola_diagnostic::{Diagnostic, DiagnosticError};

/// Warning emitted when the completed output graph contains no HTML pages.
pub fn no_pages_diagnostic(config: &crate::config::ResolvedSiteConfig) -> Diagnostic {
    let entry = crate::filesystem::display_path(&config.build.entry, config.get_root());
    Diagnostic::new(
        tola_diagnostic::codes::site::NO_PAGES,
        tola_diagnostic::Severity::Warning,
        "no HTML pages were produced",
    )
    .with_help(format!(
        "Create the site root page in `{entry}` with `#document(\"index.html\")[...]`"
    ))
}

/// Warning emitted when the completed output graph publishes no page at `404.html`.
pub(super) fn not_found_missing_diagnostic(
    config: &crate::config::ResolvedSiteConfig,
) -> Diagnostic {
    let entry = crate::filesystem::display_path(&config.build.entry, config.get_root());
    Diagnostic::new(
        tola_diagnostic::codes::site::NOT_FOUND_MISSING,
        tola_diagnostic::Severity::Warning,
        "no `404.html` page was produced",
    )
    .with_help(format!(
        "Add a page to `{entry}` with `#document(\"404.html\", format: \"html\")[...]`"
    ))
}

/// Refuse a build that would read vendored inputs a stopped `tola vendor` left half replaced.
///
/// A present swap workspace means the last replacement never finished, so the vendored trees may
/// be mixed; `tola vendor` completes it.
pub(super) fn refuse_incomplete_vendor(
    config: &crate::config::ResolvedSiteConfig,
) -> anyhow::Result<()> {
    // A vendor candidate is the tree `tola vendor` is preparing now, not an installed one a
    // stopped run left under recovery.
    if config.vendor_candidate.is_some() {
        return Ok(());
    }
    let Some(vendor_path) = config.vendor.path.as_ref() else {
        return Ok(());
    };
    if !crate::filesystem::TreeSwap::beside(config.get_root(), vendor_path)?.is_claimed()? {
        return Ok(());
    }
    let vendor = crate::filesystem::display_path(vendor_path, config.get_root());
    let diagnostic = Diagnostic::new(
        tola_diagnostic::codes::vendor::INCOMPLETE,
        tola_diagnostic::Severity::Error,
        "a previous `tola vendor` stopped before it finished",
    )
    .with_note(vendor)
    .with_help("Run `tola vendor` to complete the replacement");
    Err(anyhow::Error::new(DiagnosticError::attach(
        anyhow::anyhow!("the vendored inputs are incomplete"),
        vec![diagnostic],
    )))
}

pub(super) fn with_site(error: anyhow::Error, root: &Path) -> anyhow::Error {
    with_report(error, root, tola_diagnostic::codes::build::SITE)
}

pub(super) fn with_write(error: anyhow::Error, root: &Path) -> anyhow::Error {
    with_report(error, root, tola_diagnostic::codes::publish::SITE)
}

fn with_report(
    error: anyhow::Error,
    root: &Path,
    fallback_code: tola_diagnostic::DiagnosticCode,
) -> anyhow::Error {
    crate::compiler::diagnostic::attach_reports(error, |error| reports(error, root, fallback_code))
}

pub fn for_error(error: &anyhow::Error, root: &Path) -> Vec<Diagnostic> {
    if crate::cancellation::is_cancelled(error) {
        return Vec::new();
    }
    reports(error, root, tola_diagnostic::codes::build::SITE)
}

/// The reports one boundary hands a reader: attached by a producer, or classified here.
fn reports(
    error: &anyhow::Error,
    root: &Path,
    fallback_code: tola_diagnostic::DiagnosticCode,
) -> Vec<Diagnostic> {
    if let Some(diagnostics) = tola_diagnostic::attached(error) {
        return diagnostics.to_vec();
    }
    classify(error, root).unwrap_or_else(|| vec![tola_diagnostic::fallback(fallback_code, error)])
}

fn classify(error: &anyhow::Error, root: &Path) -> Option<Vec<Diagnostic>> {
    crate::compiler::error_diagnostics(error, root)
        .or_else(|| crate::icon::error_diagnostic(error, root).map(|diagnostic| vec![diagnostic]))
        .or_else(|| {
            crate::output::graph::error_diagnostic(error).map(|diagnostic| vec![diagnostic])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_error_keeps_its_cancellation() {
        let error = anyhow::Error::new(tola_typst::CompileError::Cancelled);
        let error = with_site(error, Path::new("/site"));
        assert!(crate::cancellation::is_cancelled(&error));
        assert!(tola_diagnostic::attached(&error).is_none());
        assert!(for_error(&error, Path::new("/site")).is_empty());
    }

    #[test]
    fn font_failure_names_the_configuration_key() {
        let error = anyhow::Error::new(tola_typst::FontLoadError::NotDirectory {
            path: std::path::PathBuf::from("/site/fonts/Arial.ttf"),
        });

        let diagnosed = with_site(error, Path::new("/site"));
        let diagnostics = for_error(&diagnosed, Path::new("/site"));

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, tola_diagnostic::codes::typst::FONT);
        assert!(diagnostics[0].message.contains("fonts/Arial.ttf"));
        assert!(!diagnostics[0].message.contains("/site"));
        assert!(!diagnostics[0].help.is_empty());
    }

    #[test]
    fn output_conflict_names_its_path() {
        let path = std::path::PathBuf::from("/site");
        let diagnostics = for_error(
            &anyhow::Error::new(crate::output::graph::OutputGraphError::ReservedPath {
                path: tola_address::OutputPath::parse("_tola/search.json").unwrap(),
                owner: crate::output::owner::OutputOwner::system("search"),
            }),
            &path,
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "site.outputs");
        assert!(
            diagnostics[0].message.contains("_tola/search.json"),
            "{}",
            diagnostics[0].message
        );
        assert_eq!(
            diagnostics[0]
                .help
                .first()
                .map(|help| help.message.as_str()),
            Some("use a path outside `_tola`")
        );
    }

    /// A build reads no vendored input while a stopped replacement waits for `tola vendor`.
    #[test]
    fn unfinished_vendor_replacement_refuses_the_build() {
        let site = tempfile::TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(
            site.path(),
            crate::config::tests::VENDORED_SITE_SOURCE,
        );
        assert!(refuse_incomplete_vendor(&config).is_ok());

        let vendor = config.vendor.path.as_ref().expect("a declared vendor path");
        crate::filesystem::TreeSwap::beside(config.get_root(), vendor)
            .unwrap()
            .claim()
            .unwrap();

        let error = refuse_incomplete_vendor(&config).unwrap_err();
        let diagnostics = tola_diagnostic::attached(&error).expect("the refusal is a diagnostic");
        assert_eq!(
            diagnostics[0].code,
            tola_diagnostic::codes::vendor::INCOMPLETE
        );
    }

    #[test]
    fn build_error_renders_outer_context_once() {
        let error = anyhow::anyhow!("disk read failed")
            .context("read source")
            .context("compile the root Bundle");

        let error = with_site(error, Path::new("/site"));
        let diagnostics = tola_diagnostic::attached(&error).unwrap();

        assert_eq!(diagnostics[0].message, "compile the root Bundle");
        // Only the outermost message reaches the reader; inner contexts stay in the chain.
        assert!(diagnostics[0].notes.is_empty());
        assert_eq!(error.chain().count(), 4);
    }
}
