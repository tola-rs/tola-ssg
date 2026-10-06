//! Diagnostic collections, counts, and selection with aligned native spans.

use std::fmt;

use typst::World;
use typst::diag::Severity;

use super::NativeDiagnostic;
use super::filter::DiagnosticFilter;
use super::message::{Diagnostic, SourceContextLimit};
use super::source::resolve_diagnostic_with_options;

/// Summary of diagnostic counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticSummary {
    /// Number of errors.
    pub errors: usize,
    /// Number of warnings.
    pub warnings: usize,
}

impl DiagnosticSummary {
    /// Create summary from diagnostics.
    pub fn from_diagnostics(diagnostics: &[NativeDiagnostic]) -> Self {
        let (errors, warnings) =
            diagnostics
                .iter()
                .fold((0, 0), |(errors, warnings), diagnostic| {
                    match diagnostic.source().severity {
                        Severity::Error => (errors + 1, warnings),
                        Severity::Warning => (errors, warnings + 1),
                    }
                });
        Self { errors, warnings }
    }

    /// Total number of diagnostics.
    pub fn total(&self) -> usize {
        self.errors + self.warnings
    }

    /// Whether there are any errors.
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }

    /// Whether there are no errors or warnings.
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

impl fmt::Display for DiagnosticSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.errors, self.warnings) {
            (0, 0) => write!(f, "no diagnostics"),
            (e, 0) => write!(f, "{e} error{}", if e == 1 { "" } else { "s" }),
            (0, w) => write!(f, "{w} warning{}", if w == 1 { "" } else { "s" }),
            (e, w) => write!(
                f,
                "{e} error{}, {w} warning{}",
                if e == 1 { "" } else { "s" },
                if w == 1 { "" } else { "s" }
            ),
        }
    }
}

/// Diagnostics with resolved source locations; rendering does not need a `World`.
///
/// # Example
///
/// ```ignore
/// let result = compile_world(&world)?;
/// let diagnostics = result.diagnostics();
///
/// // Check for warnings
/// if !diagnostics.is_empty() {
///     // One line per diagnostic, errors first
///     eprintln!("{}", diagnostics);
///
///     // Or iterate for custom handling
///     for diag in diagnostics.iter() {
///         println!("{}: {}", diag.severity, diag.message);
///     }
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    /// Resolved diagnostic info for display
    messages: Vec<Diagnostic>,
    /// Native spans and producer identity aligned with the resolved projection.
    raw: Vec<NativeDiagnostic>,
}

impl Diagnostics {
    /// Create an empty diagnostics collection.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            raw: Vec::new(),
        }
    }

    /// Append diagnostics from another compilation phase while preserving
    /// both resolved display information and original Typst diagnostics.
    pub fn extend(&mut self, other: &Self) {
        let raw_is_aligned =
            self.raw.len() == self.messages.len() && other.raw.len() == other.messages.len();
        self.messages.extend(other.messages.iter().cloned());
        if raw_is_aligned {
            self.raw.extend(other.raw.iter().cloned());
        } else {
            self.raw.clear();
        }
    }

    /// Append distinct diagnostics in encounter order, retaining native span and producer identity.
    ///
    /// Repeated native diagnostics keep their first context and combine package navigation.
    /// Without aligned native spans, only equal resolved diagnostics can be identified as repeats.
    pub fn extend_distinct(&mut self, other: &Self) {
        let raw_is_aligned =
            self.raw.len() == self.messages.len() && other.raw.len() == other.messages.len();
        for (index, diagnostic) in other.messages.iter().enumerate() {
            let repeated = if raw_is_aligned {
                let incoming = &other.raw[index];
                self.raw.iter().position(|retained| {
                    retained.origin() == incoming.origin()
                        && retained.source().span == incoming.source().span
                        && retained.source().severity == incoming.source().severity
                        && retained.source().message == incoming.source().message
                })
            } else {
                self.messages
                    .iter()
                    .position(|retained| retained == diagnostic)
            };
            if let Some(index) = repeated {
                let imported_by = &mut self.messages[index].imported_by;
                imported_by.extend(diagnostic.imported_by.iter().cloned());
                imported_by.sort();
                imported_by.dedup();
            } else {
                self.messages.push(diagnostic.clone());
                if raw_is_aligned {
                    self.raw.push(other.raw[index].clone());
                }
            }
        }
        if !raw_is_aligned {
            self.raw.clear();
        }
    }

    /// Create from a vector of diagnostic info (without raw diagnostics).
    pub fn from_vec(messages: Vec<Diagnostic>) -> Self {
        Self {
            messages,
            raw: Vec::new(),
        }
    }

    /// Resolve typed native diagnostics against the compilation's world.
    pub fn resolve<W: World>(world: &W, diagnostics: &[NativeDiagnostic]) -> Self {
        Self::resolve_with_options(world, diagnostics, SourceContextLimit::default())
    }

    /// Resolve diagnostics with explicit source context limits.
    pub fn resolve_with_options<W: World>(
        world: &W,
        diagnostics: &[NativeDiagnostic],
        context_limit: SourceContextLimit,
    ) -> Self {
        Self::resolve_owned(world, diagnostics.to_vec(), context_limit)
    }

    pub(crate) fn resolve_owned<W: World>(
        world: &W,
        diagnostics: Vec<NativeDiagnostic>,
        context_limit: SourceContextLimit,
    ) -> Self {
        let messages = diagnostics
            .iter()
            .map(|d| resolve_diagnostic_with_options(world, d, context_limit))
            .collect();
        Self {
            messages,
            raw: diagnostics,
        }
    }

    /// Check if there are no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the number of diagnostics.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.messages.iter().any(|d| d.severity == Severity::Error)
    }

    /// Check if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        self.messages
            .iter()
            .any(|d| d.severity == Severity::Warning)
    }

    /// Count errors.
    pub fn error_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }

    /// Count warnings.
    pub fn warning_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count()
    }

    /// Get a summary of diagnostic counts.
    pub fn summary(&self) -> DiagnosticSummary {
        DiagnosticSummary {
            errors: self.error_count(),
            warnings: self.warning_count(),
        }
    }

    /// Iterate over all diagnostics.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.messages.iter()
    }

    /// Iterate over errors only.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.messages
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    /// Iterate over warnings only.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.messages
            .iter()
            .filter(|d| d.severity == Severity::Warning)
    }

    /// Add static package navigation when Typst supplied no call trace.
    ///
    /// The site files contain imports or includes of the package; this does not prove those
    /// statements executed. A producer reporting after compilation reuses that compilation's hints.
    pub fn attach_imported_by(&mut self, importers: impl Fn(&str) -> Vec<String>) {
        for diagnostic in &mut self.messages {
            if !diagnostic.traces.is_empty() {
                continue;
            }
            let Some(package) = diagnostic
                .location
                .path
                .as_deref()
                .and_then(super::package_imports::package_specifier)
            else {
                continue;
            };
            diagnostic.imported_by = importers(package);
        }
    }

    /// Name the package a diagnostic imports that could not be provided.
    ///
    /// `resolve` answers with the package a diagnostic's own import statement names,
    /// so a diagnostic that is not about one keeps no package.
    pub(crate) fn attach_package_failures(
        &mut self,
        resolve: impl Fn(&NativeDiagnostic) -> Option<super::ResolvedPackageFailure>,
    ) {
        if self.raw.len() != self.messages.len() {
            return;
        }
        for (index, raw) in self.raw.iter().enumerate() {
            self.messages[index].package_failure = resolve(raw);
        }
    }

    /// Get a slice of all diagnostics.
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.messages
    }

    /// The original source spans, hints, traces, and typed producer identity.
    ///
    /// A mixed collection assembled from resolved and display-only values has no aligned native
    /// representation and returns an empty slice. Re-resolution retains every producer identity.
    pub fn raw(&self) -> &[NativeDiagnostic] {
        &self.raw
    }

    /// Filter diagnostics, keeping only those that pass the predicate.
    pub fn filter<F>(&self, predicate: F) -> Self
    where
        F: Fn(&Diagnostic) -> bool,
    {
        self.select_by(|diagnostic, _| predicate(diagnostic))
    }

    /// Filter out diagnostics matching any of the given filters.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tola_typst::{Diagnostics, DiagnosticFilter};
    ///
    /// let filtered = diagnostics.filter_out(&[
    ///     DiagnosticFilter::new(
    ///         DiagnosticSeverity::Warning,
    ///         FilterType::MessageContains("warning text".into()),
    ///     ),
    /// ]);
    /// ```
    pub fn filter_out(&self, filters: &[DiagnosticFilter]) -> Self {
        self.select_by(|diagnostic, raw| {
            !filters.iter().any(|filter| match raw {
                Some(raw) => filter.matches(raw),
                None => filter.matches_info(diagnostic),
            })
        })
    }

    /// Select in display order, retaining raw diagnostics only when all are aligned.
    fn select_by(
        &self,
        predicate: impl Fn(&Diagnostic, Option<&NativeDiagnostic>) -> bool,
    ) -> Self {
        let raw_is_aligned = self.raw.len() == self.messages.len();
        let mut selected = Self::new();
        for (index, diagnostic) in self.messages.iter().enumerate() {
            let raw = raw_is_aligned.then(|| &self.raw[index]);
            if predicate(diagnostic, raw) {
                selected.messages.push(diagnostic.clone());
                if let Some(raw) = raw {
                    selected.raw.push(raw.clone());
                }
            }
        }
        selected
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.messages.into_iter()
    }
}

/// One line per diagnostic, errors first: `path:line:column: severity: message`.
///
/// Plain text without colours or source snippets; a diagnostic renders with the location it
/// knows, `path:line`, `path`, or nothing but its message.
impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ordered: Vec<&Diagnostic> = self.messages.iter().collect();
        ordered.sort_by_key(|diagnostic| match diagnostic.severity {
            Severity::Error => 0,
            Severity::Warning => 1,
        });
        for diagnostic in ordered {
            let label = match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            match diagnostic.location.spelled() {
                Some(location) => writeln!(f, "{location}: {label}: {}", diagnostic.message)?,
                None => writeln!(f, "{label}: {}", diagnostic.message)?,
            }
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.messages.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::super::message::{SourceLocation, SourceTruncation, Trace, TraceKind};
    use super::*;
    use typst::diag::SourceDiagnostic;
    use typst::syntax::Span;

    fn unresolved_diagnostics(native: NativeDiagnostic) -> Diagnostics {
        let diagnostic = Diagnostic {
            origin: native.origin().clone(),
            severity: native.source().severity,
            message: native.source().message.to_string(),
            location: SourceLocation::unresolved(super::super::LocationFailure::SourceUnavailable),
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        Diagnostics {
            messages: vec![diagnostic],
            raw: vec![native],
        }
    }

    #[test]
    fn repeated_warnings_merge_package_navigation() {
        let warning = SourceDiagnostic::warning(Span::detached(), "missing font");
        let mut retained = unresolved_diagnostics(warning.clone().into());
        retained.messages[0].imported_by = vec!["content/b.typ".into()];
        let mut repeated = unresolved_diagnostics(warning.with_hint("another context").into());
        repeated.messages[0].imported_by = vec!["content/a.typ".into(), "content/b.typ".into()];

        retained.extend_distinct(&repeated);

        assert_eq!(retained.len(), 1);
        assert_eq!(retained.raw().len(), 1);
        assert!(retained.raw()[0].source().hints.is_empty());
        assert_eq!(
            retained.messages[0].imported_by,
            ["content/a.typ", "content/b.typ"]
        );
    }

    #[test]
    fn distinct_native_issues_remain_separate() {
        use typst::syntax::{DiagSpan, RootedPath, VirtualPath, VirtualRoot};

        let source = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("content/page.typ").unwrap(),
        )
        .intern();
        let first = DiagSpan::from_range(source, 0..1);
        let second = DiagSpan::from_range(source, 2..3);
        let native: [NativeDiagnostic; 6] = [
            SourceDiagnostic::warning(first, "warning").into(),
            SourceDiagnostic::warning(second, "warning").into(),
            SourceDiagnostic::error(first, "warning").into(),
            SourceDiagnostic::warning(first, "different warning").into(),
            super::super::producer_warning(Span::detached(), "image", &[("path", "a")], "warning")
                .into(),
            super::super::producer_warning(Span::detached(), "image", &[("path", "b")], "warning")
                .into(),
        ];
        let mut diagnostics = Diagnostics::new();
        for warning in &native {
            diagnostics.extend_distinct(&unresolved_diagnostics(warning.clone()));
        }
        diagnostics.extend_distinct(&diagnostics.clone());
        assert_eq!(diagnostics.len(), native.len());
        assert_eq!(diagnostics.raw(), &native);
    }

    #[test]
    fn resolved_only_repeats_require_equal_context() {
        let warning = SourceDiagnostic::warning(Span::detached(), "warning");
        let mut first = unresolved_diagnostics(warning.into());
        first.raw.clear();
        let mut second = first.clone();
        second.messages[0].imported_by = vec!["content/page.typ".into()];

        first.extend_distinct(&second);
        first.extend_distinct(&second);

        assert_eq!(first.len(), 2);
        assert!(first.raw().is_empty());
    }

    #[test]
    fn diagnostic_summary_counts_each_severity() {
        let warnings = [
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::warning(Span::detached(), "warning 2"),
        ]
        .map(NativeDiagnostic::from);
        let mixed = [
            SourceDiagnostic::warning(Span::detached(), "warning 1"),
            SourceDiagnostic::error(Span::detached(), "error 1"),
        ]
        .map(NativeDiagnostic::from);
        type SummaryCase<'a> = (&'a str, &'a [NativeDiagnostic], (usize, usize, bool));
        let cases: [SummaryCase<'_>; 3] = [
            ("empty", &[], (0, 0, false)),
            ("warnings only", &warnings, (0, 2, false)),
            ("mixed", &mixed, (1, 1, true)),
        ];
        for (name, diagnostics, (errors, warnings, has_error)) in cases {
            let summary = DiagnosticSummary::from_diagnostics(diagnostics);
            assert_eq!(
                (summary.errors, summary.warnings),
                (errors, warnings),
                "{name}"
            );
            assert_eq!(summary.has_errors(), has_error, "{name}");
        }
    }

    #[test]
    fn display_spells_each_known_location() {
        let warning = |path: Option<&str>, line: Option<usize>, column: Option<usize>| Diagnostic {
            origin: crate::diagnostic::DiagnosticOrigin::Typst,
            severity: Severity::Warning,
            message: "layout was ignored during HTML export".into(),
            location: SourceLocation {
                path: path.map(str::to_owned),
                line,
                column,
                range: None,
                source_lines: Vec::new(),
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        let diagnostics = Diagnostics::from_vec(vec![
            warning(Some("content/post.typ"), Some(4), Some(7)),
            warning(Some("content/post.typ"), Some(4), None),
            warning(Some("content/post.typ"), None, None),
            warning(None, None, None),
        ]);

        assert_eq!(
            diagnostics.to_string(),
            "content/post.typ:4:7: warning: layout was ignored during HTML export\n\
             content/post.typ:4: warning: layout was ignored during HTML export\n\
             content/post.typ: warning: layout was ignored during HTML export\n\
             warning: layout was ignored during HTML export\n"
        );
    }

    #[test]
    fn package_locations_name_importing_files() {
        fn info(path: &str, traces: Vec<Trace>) -> Diagnostic {
            Diagnostic {
                origin: crate::diagnostic::DiagnosticOrigin::Typst,
                severity: Severity::Warning,
                message: "layout was ignored during HTML export".into(),
                location: SourceLocation {
                    path: Some(path.into()),
                    line: Some(18),
                    column: Some(91),
                    range: None,
                    source_lines: Vec::new(),
                    location_failure: None,
                    truncation: SourceTruncation::default(),
                },
                hints: Vec::new(),
                traces,
                imported_by: Vec::new(),
                package_failure: None,
            }
        }
        let trace = Trace {
            kind: TraceKind::Import("templates/page.typ".into()),
            message: "while importing templates/page.typ".into(),
            location: SourceLocation {
                path: Some("templates/page.typ".into()),
                line: Some(1),
                column: Some(1),
                range: None,
                source_lines: Vec::new(),
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
        };
        let mut diagnostics = Diagnostics::from_vec(vec![
            info("@preview/cetz:0.3.4/src/canvas.typ", Vec::new()),
            info("@preview/cetz:0.3.4/src/canvas.typ", vec![trace]),
            info("content/index.typ", Vec::new()),
        ]);

        diagnostics.attach_imported_by(|package| vec![package.to_owned()]);

        assert_eq!(
            diagnostics.as_slice()[0].imported_by,
            ["@preview/cetz:0.3.4"]
        );
        assert!(diagnostics.as_slice()[1].imported_by.is_empty());
        assert!(diagnostics.as_slice()[2].imported_by.is_empty());
    }

    #[test]
    fn filtering_keeps_matched_messages() {
        let info = |message: &str| Diagnostic {
            origin: crate::diagnostic::DiagnosticOrigin::Typst,
            severity: Severity::Warning,
            message: message.into(),
            location: SourceLocation {
                path: None,
                line: None,
                column: None,
                range: None,
                source_lines: Vec::new(),
                location_failure: None,
                truncation: SourceTruncation::default(),
            },
            hints: Vec::new(),
            traces: Vec::new(),
            imported_by: Vec::new(),
            package_failure: None,
        };
        let diagnostics = Diagnostics {
            messages: vec![info("keep"), info("drop")],
            raw: vec![SourceDiagnostic::warning(Span::detached(), "raw only").into()],
        };
        let filter = crate::diagnostic::DiagnosticFilter::new(
            Severity::Warning,
            crate::diagnostic::FilterType::MessageContains("drop".into()),
        );

        let filtered = diagnostics.filter_out(&[filter]);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.as_slice()[0].message, "keep");
        assert!(filtered.raw().is_empty());
    }
}
