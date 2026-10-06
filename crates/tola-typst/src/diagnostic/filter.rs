//! Diagnostic filtering utilities.

use super::NativeDiagnostic;
use typst::diag::Severity;

const HTML_EXPORT_WARNING: &str = "html export is under active development and incomplete";
const BUNDLE_EXPORT_WARNING: &str = "bundle export is experimental";

/// Specifies which packages to filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageKind {
    /// All preview packages (`@preview/*`).
    AllPreview,
    /// All local packages (`@local/*`).
    AllLocal,
    /// Specific packages by full path.
    ///
    /// - `"@preview/cetz"` matches cetz from preview namespace
    /// - `"@local/mylib"` matches mylib from local namespace
    /// - `"@myapp"` matches all myapp packages
    Specific(Vec<String>),
}

impl PackageKind {
    /// Create a filter for specific packages.
    pub fn specific<I, S>(packages: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::Specific(
            packages
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        )
    }

    /// Check whether `package`, written as `@{namespace}/{name}`, matches this filter.
    fn matches_package(&self, package: &str) -> bool {
        match self {
            PackageKind::AllPreview => package.starts_with("@preview/"),
            PackageKind::AllLocal => package.starts_with("@local/"),
            PackageKind::Specific(patterns) => patterns
                .iter()
                .any(|pattern| package_pattern_matches(package, pattern)),
        }
    }
}

/// Match a package namespace or package name without confusing a name prefix
/// with a different package (for example, `cetz` versus `cetz-extra`).
fn package_pattern_matches(path: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    path == pattern
        || path
            .strip_prefix(pattern)
            .is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with(':'))
}

/// Filter type for matching diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterType {
    /// Match all diagnostics.
    All,
    /// Match diagnostics from specific packages.
    Package(PackageKind),
    /// Match diagnostics containing specific text in message.
    MessageContains(String),
    /// Match detached diagnostics containing specific text in message.
    DetachedMessageContains(String),
}

/// The facts a filter matches against, read from either diagnostic representation.
struct FilterSubject<'a> {
    /// Originating package, written as `@{namespace}/{name}`.
    package: Option<String>,
    /// Diagnostic message.
    message: &'a str,
    /// Whether the diagnostic has no source location.
    detached: bool,
}

impl<'a> FilterSubject<'a> {
    fn of_native(native: &'a NativeDiagnostic) -> Self {
        let diagnostic = native.source();
        Self {
            package: diagnostic.span.id().and_then(|id| match id.root() {
                typst::syntax::VirtualRoot::Package(package) => {
                    Some(format!("@{}/{}", package.namespace, package.name))
                }
                typst::syntax::VirtualRoot::Project => None,
            }),
            message: &diagnostic.message,
            detached: diagnostic.span.is_detached(),
        }
    }

    /// Resolution names package files as `@namespace/name:version/file`, so a
    /// package is identified by the path prefix before its file.
    fn of_resolved(diagnostic: &'a super::message::Diagnostic) -> Self {
        Self {
            package: diagnostic
                .location
                .path
                .as_deref()
                .and_then(package_of_path),
            message: &diagnostic.message,
            detached: diagnostic.location.path.is_none(),
        }
    }
}

/// Extract `@{namespace}/{name}` from a rendered package path.
fn package_of_path(path: &str) -> Option<String> {
    let mut segments = path.splitn(3, '/');
    let namespace = segments.next()?;
    let name = segments.next()?;
    (namespace.starts_with('@') && !name.is_empty()).then(|| format!("{namespace}/{name}"))
}

impl FilterType {
    fn matches(&self, subject: FilterSubject<'_>) -> bool {
        match self {
            FilterType::All => true,
            FilterType::Package(kind) => subject
                .package
                .is_some_and(|package| kind.matches_package(&package)),
            FilterType::MessageContains(text) => subject.message.contains(text.as_str()),
            FilterType::DetachedMessageContains(text) => {
                subject.detached && subject.message.contains(text.as_str())
            }
        }
    }

    fn matches_native(&self, diagnostic: &NativeDiagnostic) -> bool {
        self.matches(FilterSubject::of_native(diagnostic))
    }

    fn matches_info(&self, diagnostic: &super::message::Diagnostic) -> bool {
        self.matches(FilterSubject::of_resolved(diagnostic))
    }
}

/// Exclude diagnostics by severity and filter type.
///
/// # Example
///
/// ```ignore
/// use tola_typst::typst::diag::Severity;
/// use tola_typst::diagnostic::{DiagnosticFilter, FilterType, PackageKind};
///
/// // Filter all warnings
/// let filter = DiagnosticFilter::new(Severity::Warning, FilterType::All);
///
/// // Filter errors from specific packages
/// let filter = DiagnosticFilter::new(
///     Severity::Error,
///     FilterType::Package(PackageKind::specific(["@myapp/pages"])),
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticFilter {
    /// The severity to match (Error or Warning).
    pub severity: Severity,
    /// The filter type to apply.
    pub filter: FilterType,
}

impl DiagnosticFilter {
    /// Create a new diagnostic filter.
    pub fn new(severity: Severity, filter: FilterType) -> Self {
        Self { severity, filter }
    }

    /// Create the filter for Typst's non-actionable HTML export warning.
    pub fn html_export_warning_filter() -> Self {
        Self::new(
            Severity::Warning,
            FilterType::DetachedMessageContains(HTML_EXPORT_WARNING.into()),
        )
    }

    /// Create the filter for Typst's non-actionable Bundle export warning.
    pub fn bundle_export_warning_filter() -> Self {
        Self::new(
            Severity::Warning,
            FilterType::DetachedMessageContains(BUNDLE_EXPORT_WARNING.into()),
        )
    }

    /// Return the standard Typst export warnings that integrations may omit at the boundary
    /// where a site author reads diagnostics.
    pub fn typst_export_warning_filters() -> [Self; 2] {
        [
            Self::html_export_warning_filter(),
            Self::bundle_export_warning_filter(),
        ]
    }

    /// Check if a typed native diagnostic should be filtered out.
    pub fn matches(&self, diag: &NativeDiagnostic) -> bool {
        diag.source().severity == self.severity && self.filter.matches_native(diag)
    }

    pub(crate) fn matches_info(&self, diagnostic: &super::message::Diagnostic) -> bool {
        diagnostic.severity == self.severity && self.filter.matches_info(diagnostic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::diag::SourceDiagnostic;
    use typst::syntax::Span;

    #[test]
    fn filter_keeps_unmatched_diagnostics() {
        let diagnostics = [
            SourceDiagnostic::error(Span::detached(), "error with keyword"),
            SourceDiagnostic::error(Span::detached(), "other error"),
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::warning(Span::detached(), "warning 2"),
        ]
        .map(NativeDiagnostic::from);
        for (name, severity, filter, expected) in [
            (
                "all warnings",
                Severity::Warning,
                FilterType::All,
                &["error with keyword", "other error"][..],
            ),
            (
                "errors naming a keyword",
                Severity::Error,
                FilterType::MessageContains("keyword".into()),
                &["other error", "warning 1", "warning 2"][..],
            ),
        ] {
            let filter = DiagnosticFilter::new(severity, filter);
            let kept: Vec<&str> = diagnostics
                .iter()
                .filter(|diagnostic| !filter.matches(diagnostic))
                .map(|diagnostic| diagnostic.source().message.as_str())
                .collect();
            assert_eq!(kept, expected, "{name}");
        }
    }

    /// A world whose library enables the export features, as every world this crate builds does.
    fn feature_enabled_world(source: &str) -> (tempfile::TempDir, crate::world::TypstWorld) {
        let directory = tempfile::TempDir::new().unwrap();
        let main = directory.path().join("site.typ");
        std::fs::write(&main, source).unwrap();
        let world = crate::world::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .expect("valid test world");
        (directory, world)
    }

    fn removed_export_warnings(warnings: &[NativeDiagnostic]) -> Vec<String> {
        let filters = DiagnosticFilter::typst_export_warning_filters();
        warnings
            .iter()
            .filter(|warning| {
                assert_eq!(warning.source().severity, Severity::Warning);
                filters.iter().any(|filter| filter.matches(warning))
            })
            .map(|warning| warning.source().message.to_string())
            .collect()
    }

    #[test]
    fn export_filters_match_typst_warnings() {
        // Typst emits the HTML warning from the single-document path and the Bundle warning
        // from the Bundle path; if it rewords either, this test fails rather than a host build.
        let (_html_directory, html_world) = feature_enabled_world("= Heading\n\nBody");
        let html = crate::compile::compile_world(&html_world).expect("trivial document compiles");
        assert!(
            removed_export_warnings(html.diagnostics().raw())
                .contains(&HTML_EXPORT_WARNING.to_owned()),
            "the html export warning is no longer removed; Typst reworded it, so update the constant"
        );

        let (_bundle_directory, bundle_world) =
            feature_enabled_world(r#"#document("index.html")[Site]"#);
        let bundle = typst::compile::<typst_bundle::Bundle>(&bundle_world)
            .warnings
            .into_iter()
            .map(NativeDiagnostic::from)
            .collect::<Vec<_>>();
        assert!(
            removed_export_warnings(&bundle).contains(&BUNDLE_EXPORT_WARNING.to_owned()),
            "the bundle export warning is no longer removed; Typst reworded it, so update the constant"
        );

        let error = NativeDiagnostic::from(SourceDiagnostic::error(
            Span::detached(),
            BUNDLE_EXPORT_WARNING,
        ));
        let filters = DiagnosticFilter::typst_export_warning_filters();
        assert!(!filters.iter().any(|filter| filter.matches(&error)));
    }

    #[test]
    fn package_filter_matches_specifier() {
        fn info(path: &str) -> super::super::message::Diagnostic {
            super::super::message::Diagnostic {
                origin: crate::diagnostic::DiagnosticOrigin::Typst,
                severity: Severity::Error,
                message: String::new(),
                location: super::super::message::SourceLocation {
                    path: Some(path.into()),
                    line: None,
                    column: None,
                    range: None,
                    source_lines: Vec::new(),
                    location_failure: None,
                    truncation: Default::default(),
                },
                hints: Vec::new(),
                traces: Vec::new(),
                imported_by: Vec::new(),
                package_failure: None,
            }
        }

        let filter = DiagnosticFilter::new(
            Severity::Error,
            FilterType::Package(PackageKind::specific(["@preview/cetz"])),
        );
        assert!(filter.matches_info(&info("@preview/cetz")));
        assert!(filter.matches_info(&info("@preview/cetz:0.1.0")));
        assert!(filter.matches_info(&info("@preview/cetz/lib.typ")));
        assert!(!filter.matches_info(&info("@preview/cetz-extra")));
    }
}
