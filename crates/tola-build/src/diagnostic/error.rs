//! Structured diagnostics through ordinary error chains.

use super::{Diagnostic, Severity};

#[derive(Debug)]
pub struct DiagnosticError {
    summary: String,
    diagnostics: Vec<Diagnostic>,
    source: Option<anyhow::Error>,
}

impl DiagnosticError {
    pub fn new(summary: impl Into<String>, diagnostics: Vec<Diagnostic>) -> Self {
        assert!(
            !diagnostics.is_empty(),
            "a diagnostic error must hold at least one diagnostic"
        );
        Self {
            summary: summary.into(),
            diagnostics,
            source: None,
        }
    }

    pub fn attach(source: anyhow::Error, diagnostics: Vec<Diagnostic>) -> Self {
        assert!(
            !diagnostics.is_empty(),
            "a diagnostic error must hold at least one diagnostic"
        );
        Self {
            summary: source.to_string(),
            diagnostics,
            source: Some(source),
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl std::error::Error for DiagnosticError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|error| error.as_ref())
    }
}

/// Find diagnostics attached through [`DiagnosticError`] in the error chain.
pub fn attached(error: &anyhow::Error) -> Option<&[Diagnostic]> {
    error
        .downcast_ref::<DiagnosticError>()
        .or_else(|| {
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<DiagnosticError>())
        })
        .map(DiagnosticError::diagnostics)
}

/// Convert an unclassified error to a diagnostic the site author reads.
///
/// Only the outermost message is retained: it is the sentence the failing operation wrote for its
/// reader. Inner causes describe Tola's own mechanics and stay out of rendered output; command
/// boundaries record the complete chain to their debug log instead.
pub fn fallback(code: super::DiagnosticCode, error: &anyhow::Error) -> Diagnostic {
    let message = error
        .chain()
        .next()
        .map(ToString::to_string)
        .unwrap_or_default();
    Diagnostic::new(code, Severity::Error, collapse_whitespace(&message))
}

/// Join a message onto one line so a header stays readable when a cause spans several lines.
fn collapse_whitespace(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic_error() -> DiagnosticError {
        DiagnosticError::new(
            "source analysis failed",
            vec![Diagnostic::new(
                crate::codes::typst::SOURCE_ANALYSIS,
                Severity::Error,
                "source could not be analyzed",
            )],
        )
    }

    #[test]
    fn attached_finds_nested_diagnostics() {
        #[derive(Debug, thiserror::Error)]
        #[error("read source failed")]
        struct ReadFailure {
            #[source]
            source: DiagnosticError,
        }

        let direct = anyhow::Error::new(diagnostic_error());
        let below_contexts = anyhow::anyhow!("disk read failed")
            .context("read source")
            .context(diagnostic_error())
            .context("inspect sources");
        let through_source = anyhow::Error::new(ReadFailure {
            source: diagnostic_error(),
        });

        for error in [direct, below_contexts, through_source] {
            let diagnostics = attached(&error).expect("diagnostics should remain attached");

            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, "typst.source_analysis");
        }
    }

    #[test]
    fn fallback_uses_outermost_message() {
        let cases: [(&str, anyhow::Error, &str); 2] = [
            (
                "inner cause",
                anyhow::anyhow!("could not write output").context("publishing failed"),
                "publishing failed",
            ),
            (
                "repeated message",
                anyhow::anyhow!("same failure").context("same failure"),
                "same failure",
            ),
        ];

        for (label, error, expected) in cases {
            let diagnostic = fallback(crate::codes::typst::COMPILE, &error);

            assert_eq!(diagnostic.code, "typst.compile", "{label}");
            assert_eq!(diagnostic.message, expected, "{label}");
        }
    }

    #[test]
    fn attach_keeps_the_typed_failure() {
        #[derive(Debug, thiserror::Error)]
        #[error("disk read failed")]
        struct DiskReadFailure;

        let error = anyhow::Error::new(DiagnosticError::attach(
            anyhow::Error::new(DiskReadFailure),
            diagnostic_error().diagnostics,
        ));

        assert!(error.chain().any(|cause| cause.is::<DiskReadFailure>()));
    }
}
