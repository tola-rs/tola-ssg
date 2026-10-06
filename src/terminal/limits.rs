//! How many diagnostics the terminal shows, and what it says about the rest.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tola_build::diagnostic::{Diagnostic, Severity};

use super::Palette;
use super::diagnostic;
use super::progress;
use super::progress::plural_count;
use super::source::SourceFiles;

/// The display limits one terminal applies, shared by every clone of it.
///
/// A limit of [`usize::MAX`] shows every diagnostic of that severity; [`Self::set`] replaces both.
#[derive(Clone)]
pub(crate) struct DiagnosticLimits {
    warning_limit: Arc<AtomicUsize>,
    error_limit: Arc<AtomicUsize>,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            warning_limit: Arc::new(AtomicUsize::new(usize::MAX)),
            error_limit: Arc::new(AtomicUsize::new(usize::MAX)),
        }
    }
}

impl DiagnosticLimits {
    /// Apply the limits of the configuration this command loaded; `None` keeps every diagnostic of
    /// that severity.
    pub(crate) fn set(&self, max_errors: Option<usize>, max_warnings: Option<usize>) {
        self.warning_limit
            .store(max_warnings.unwrap_or(usize::MAX), Ordering::Relaxed);
        self.error_limit
            .store(max_errors.unwrap_or(usize::MAX), Ordering::Relaxed);
    }

    /// Render the diagnostics the limits show, followed by one notice per hidden severity.
    /// The borrowed display view puts errors first; stored diagnostics keep producer order.
    pub(crate) fn render(
        &self,
        diagnostics: &[Diagnostic],
        sources: Option<&SourceFiles>,
        palette: Palette,
    ) -> String {
        if diagnostics.is_empty() {
            return String::new();
        }
        let warning_limit = self.warning_limit.load(Ordering::Relaxed);
        let error_limit = self.error_limit.load(Ordering::Relaxed);
        let mut warnings = 0usize;
        let mut errors = 0usize;
        let mut rendered = [Severity::Error, Severity::Warning]
            .into_iter()
            .flat_map(|severity| {
                diagnostics
                    .iter()
                    .filter(move |diagnostic| diagnostic.severity == severity)
            })
            .filter(|diagnostic| match diagnostic.severity {
                Severity::Warning => {
                    warnings += 1;
                    warnings <= warning_limit
                }
                Severity::Error => {
                    errors += 1;
                    errors <= error_limit
                }
            })
            .map(|diagnostic| diagnostic::render(diagnostic, sources, palette.uses_color()))
            .collect::<Vec<_>>()
            .join("\n");
        for (hidden, severity) in [
            (errors.saturating_sub(error_limit), "error"),
            (warnings.saturating_sub(warning_limit), "warning"),
        ] {
            if hidden != 0 {
                if !rendered.is_empty() {
                    rendered.push('\n');
                }
                rendered.push_str(&progress::notice(
                    &format!(
                        "{} not shown; increase `diagnostics.max_{severity}s` to show more",
                        plural_count(hidden, severity)
                    ),
                    palette,
                ));
            }
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::codes::typst::COMPILE;

    #[test]
    fn errors_lead_each_limited_batch() {
        let diagnostics = [
            (Severity::Warning, "first warning"),
            (Severity::Error, "first error"),
            (Severity::Warning, "second warning"),
            (Severity::Error, "second error"),
        ]
        .map(|(severity, message)| Diagnostic::new(COMPILE, severity, message));
        let limits = DiagnosticLimits::default();
        for maximum in [None, Some(1), Some(0)] {
            limits.set(maximum, maximum);
            let rendered = limits.render(&diagnostics, None, Palette::new(false));
            match maximum {
                None => {
                    let positions = [
                        "first error",
                        "second error",
                        "first warning",
                        "second warning",
                    ]
                    .map(|message| rendered.find(message).unwrap());
                    assert!(
                        positions.windows(2).all(|pair| pair[0] < pair[1]),
                        "{rendered}"
                    );
                    assert!(!rendered.contains("not shown"), "{rendered}");
                }
                Some(1) => {
                    assert!(
                        rendered.find("first error").unwrap()
                            < rendered.find("first warning").unwrap(),
                        "{rendered}"
                    );
                    assert!(!rendered.contains("second error"), "{rendered}");
                    assert!(!rendered.contains("second warning"), "{rendered}");
                    assert!(rendered.contains("1 error not shown"), "{rendered}");
                    assert!(rendered.contains("1 warning not shown"), "{rendered}");
                }
                Some(0) => {
                    for diagnostic in &diagnostics {
                        assert!(!rendered.contains(&diagnostic.message), "{rendered}");
                    }
                    assert!(rendered.contains("2 errors not shown"), "{rendered}");
                    assert!(rendered.contains("2 warnings not shown"), "{rendered}");
                }
                Some(_) => unreachable!(),
            }
        }
    }
}
