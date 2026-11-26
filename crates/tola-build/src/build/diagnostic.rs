//! Diagnostics attached at complete build and output-write boundaries.

use std::path::Path;

use tola_diagnostic::{Diagnostic, DiagnosticError};

/// Warning emitted when the completed output graph contains no HTML pages.
pub fn no_pages_diagnostic(config: &crate::config::SiteConfig) -> Diagnostic {
    let entry = tola_diagnostic::display_path(&config.build.entry, config.root());
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
pub(super) fn not_found_missing_diagnostic(config: &crate::config::SiteConfig) -> Diagnostic {
    let entry = tola_diagnostic::display_path(&config.build.entry, config.root());
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
pub(super) fn refuse_incomplete_vendor(config: &crate::config::SiteConfig) -> anyhow::Result<()> {
    // A vendor candidate is the tree `tola vendor` is preparing now, not an installed one a
    // stopped run left under recovery.
    if config.vendor_candidate.is_some() {
        return Ok(());
    }
    let Some(vendor_path) = config.vendor.path.as_ref() else {
        return Ok(());
    };
    if !crate::filesystem::TreeSwap::beside(config.root(), vendor_path)?.is_claimed()? {
        return Ok(());
    }
    let vendor = tola_diagnostic::display_path(vendor_path, config.root());
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
pub(super) fn reports(
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
        .or_else(|| tola_module::error_diagnostic(error).map(|diagnostic| vec![diagnostic]))
        .or_else(|| crate::icon::error_diagnostic(error, root).map(|diagnostic| vec![diagnostic]))
        .or_else(|| {
            crate::output::graph::error_diagnostic(error).map(|diagnostic| vec![diagnostic])
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_error_stays_cancelled() {
        let err = anyhow::Error::new(tola_typst::CompileError::Cancelled);
        let err = with_site(err, Path::new("/site"));
        assert!(crate::cancellation::is_cancelled(&err));
        assert!(tola_diagnostic::attached(&err).is_none());
        assert!(for_error(&err, Path::new("/site")).is_empty());
    }

    #[test]
    fn font_failure_cites_config_key() {
        let err = anyhow::Error::new(tola_typst::FontLoadError::NotDirectory {
            path: std::path::PathBuf::from("/site/fonts/Arial.ttf"),
        });

        let diagnosed = with_site(err, Path::new("/site"));
        let diag = for_error(&diagnosed, Path::new("/site"));

        assert_eq!(diag.len(), 1);
        assert_eq!(diag[0].code, tola_diagnostic::codes::typst::FONT);
        assert!(diag[0].message.contains("fonts/Arial.ttf"));
        assert!(!diag[0].message.contains("/site"));
        assert!(!diag[0].help.is_empty());
    }

    #[test]
    fn invalid_manifest_reports_diagnostic() {
        let site = tempfile::tempdir().unwrap();
        let root = site.path();
        let dir = root.join("theme");
        std::fs::create_dir(&dir).unwrap();
        let manifest = dir.join("tola-module.toml");
        std::fs::write(&manifest, "# Module\n[buid]\n").unwrap();
        let cfg = crate::config::tests::load_test_config(root, "[modules.theme]\npath = 'theme'\n");

        let Err(failure) = super::super::build_site(&cfg, crate::build::BuildMode::Production)
        else {
            panic!("the module manifest should fail before compilation");
        };
        let err = anyhow::Error::new(failure);
        let [diag] = tola_diagnostic::attached(&err).unwrap() else {
            panic!("expected a single manifest diagnostic");
        };
        assert_eq!(diag.code, tola_diagnostic::codes::module::MANIFEST);
        assert!(diag.message.contains("`buid`"));
        assert!(diag.help.iter().any(|h| h.message.contains("`build`")));
        let loc = diag.location.as_ref().unwrap();
        assert_eq!(
            loc.file.as_deref(),
            Some(manifest.canonicalize().unwrap().as_path())
        );
        assert_eq!(loc.range.unwrap().start.line, 1);
    }

    #[test]
    fn incomplete_vendor_refuses_build() {
        let site = tempfile::TempDir::new().unwrap();
        let cfg = crate::config::tests::load_test_config(
            site.path(),
            crate::config::tests::VENDORED_SITE_SOURCE,
        );
        assert!(refuse_incomplete_vendor(&cfg).is_ok());

        let vendor = cfg.vendor.path.as_ref().expect("a declared vendor path");
        crate::filesystem::TreeSwap::beside(cfg.root(), vendor)
            .unwrap()
            .claim()
            .unwrap();

        let err = refuse_incomplete_vendor(&cfg).unwrap_err();
        let diag = tola_diagnostic::attached(&err).expect("the refusal is a diagnostic");
        assert_eq!(diag[0].code, tola_diagnostic::codes::vendor::INCOMPLETE);
    }
}
