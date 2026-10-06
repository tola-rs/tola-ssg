//! Diagnostics attached at complete build and output-write boundaries.

use std::path::Path;

use crate::diagnostic::{Diagnostic, DiagnosticError};

/// Warning emitted when the completed output graph contains no HTML pages.
pub fn no_pages_diagnostic(config: &crate::config::ResolvedSiteConfig) -> Diagnostic {
    let entry = crate::filesystem::display_path(&config.build.entry, config.get_root());
    Diagnostic::new(
        crate::codes::site::NO_PAGES,
        crate::diagnostic::Severity::Warning,
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
        crate::codes::site::NOT_FOUND_MISSING,
        crate::diagnostic::Severity::Warning,
        "no `404.html` page was produced",
    )
    .with_help(format!(
        "Add a page to `{entry}` with `#document(\"404.html\", format: \"html\")[...]`"
    ))
}

/// Refuse a build that would read a vendor replacement this process never finished.
///
/// `tola vendor` journals its replacement before the first tree moves and marks it committed
/// after the last. A process that dies in between leaves the vendored trees mixed or short while
/// the operating system releases the site lock, so the next build would silently consume the
/// partial set. Recovery belongs to `tola vendor`, which owns the backup trees.
pub(super) fn refuse_incomplete_vendor(
    config: &crate::config::ResolvedSiteConfig,
) -> anyhow::Result<()> {
    let Some(workspace) = config.vendor.workspace_path() else {
        return Ok(());
    };
    if workspace.join("committed").exists() || !workspace.join("replacing").exists() {
        return Ok(());
    }
    let vendor = config
        .vendor
        .path
        .as_ref()
        .map(|path| crate::filesystem::display_path(path, config.get_root()))
        .unwrap_or_default();
    let diagnostic = Diagnostic::new(
        crate::codes::vendor::INCOMPLETE,
        crate::diagnostic::Severity::Error,
        "a previous `tola vendor` stopped before it finished",
    )
    .with_note(vendor)
    .with_help("Run `tola vendor` to finish or undo that replacement");
    Err(anyhow::Error::new(DiagnosticError::attach(
        anyhow::anyhow!("the vendored inputs are incomplete"),
        vec![diagnostic],
    )))
}

pub(super) fn with_site(error: anyhow::Error, root: &Path) -> anyhow::Error {
    if crate::cancellation::is_cancelled(&error) || crate::diagnostic::attached(&error).is_some() {
        return error;
    }
    attach(error, root, crate::codes::build::SITE)
}

pub(super) fn with_write(error: anyhow::Error, root: &Path) -> anyhow::Error {
    if crate::cancellation::is_cancelled(&error) || crate::diagnostic::attached(&error).is_some() {
        return error;
    }
    attach(error, root, crate::codes::publish::SITE)
}

fn attach(
    error: anyhow::Error,
    root: &Path,
    fallback_code: crate::diagnostic::DiagnosticCode,
) -> anyhow::Error {
    let diagnostics = classify(&error, root)
        .unwrap_or_else(|| vec![crate::diagnostic::fallback(fallback_code, &error)]);
    anyhow::Error::new(DiagnosticError::attach(error, diagnostics))
}

pub fn for_error(error: &anyhow::Error, root: &Path) -> Vec<Diagnostic> {
    if crate::cancellation::is_cancelled(error) {
        return Vec::new();
    }
    if let Some(diagnostics) = crate::diagnostic::attached(error) {
        return diagnostics.to_vec();
    }
    classify(error, root).unwrap_or_else(|| {
        vec![crate::diagnostic::fallback(
            crate::codes::build::SITE,
            error,
        )]
    })
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
        assert!(crate::diagnostic::attached(&error).is_none());
        assert!(for_error(&error, Path::new("/site")).is_empty());
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

    #[test]
    fn build_error_renders_outer_context_once() {
        let error = anyhow::anyhow!("disk read failed")
            .context("read source")
            .context("compile the root Bundle");

        let error = with_site(error, Path::new("/site"));
        let diagnostics = crate::diagnostic::attached(&error).unwrap();

        assert_eq!(diagnostics[0].message, "compile the root Bundle");
        // Only the outermost message reaches the reader; inner contexts stay in the chain.
        assert!(diagnostics[0].notes.is_empty());
        assert_eq!(error.chain().count(), 4);
    }
}
