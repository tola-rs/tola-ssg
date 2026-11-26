//! Structured diagnostics through ordinary error chains.

use super::{Diagnostic, Severity};

#[derive(Debug, thiserror::Error)]
#[error("{summary}")]
pub struct DiagnosticError {
    summary: String,
    diagnostics: Vec<Diagnostic>,
    source: Option<anyhow::Error>,
}

impl DiagnosticError {
    pub fn new(summary: impl Into<String>, mut diags: Vec<Diagnostic>) -> Self {
        assert!(
            !diags.is_empty(),
            "a diagnostic error must hold at least one diagnostic"
        );
        super::coalesce_reports(&mut diags);
        Self {
            summary: summary.into(),
            diagnostics: diags,
            source: None,
        }
    }

    pub fn attach(src: anyhow::Error, diags: Vec<Diagnostic>) -> Self {
        let summary = src.to_string();
        Self {
            source: Some(src),
            ..Self::new(summary, diags)
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

pub fn attached(err: &anyhow::Error) -> Option<&[Diagnostic]> {
    err.downcast_ref::<DiagnosticError>()
        .or_else(|| {
            err.chain()
                .find_map(|c| c.downcast_ref::<DiagnosticError>())
        })
        .map(DiagnosticError::diagnostics)
}

pub fn fallback(code: super::DiagnosticCode, err: &anyhow::Error) -> Diagnostic {
    Diagnostic::new(code, Severity::Error, collapse_whitespace(&err.to_string()))
}

pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag_err() -> DiagnosticError {
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
    fn nested_diagnostics() {
        #[derive(Debug, thiserror::Error)]
        #[error("read source failed")]
        struct ReadFailure {
            #[source]
            source: DiagnosticError,
        }

        let direct = anyhow::Error::new(diag_err());
        let below_ctx = anyhow::anyhow!("disk read failed")
            .context("read source")
            .context(diag_err())
            .context("inspect sources");
        let via_source = anyhow::Error::new(ReadFailure { source: diag_err() });

        for err in [direct, below_ctx, via_source] {
            let diags = attached(&err).expect("diagnostics should remain attached");

            assert_eq!(diags.len(), 1);
            assert_eq!(diags[0].code, "typst.source_analysis");
        }
    }

    #[test]
    fn typed_cause() {
        #[derive(Debug, thiserror::Error)]
        #[error("the staging directory is gone")]
        struct StagingDirectoryMissing;

        let diags = vec![
            Diagnostic::new(
                crate::codes::typst::SOURCE_ANALYSIS,
                Severity::Error,
                "the check stopped before the site was analyzed",
            )
            .with_note("the previous revision stays published"),
        ];

        let err = anyhow::Error::new(DiagnosticError::attach(
            anyhow::Error::new(StagingDirectoryMissing),
            diags,
        ));

        assert!(err.chain().any(|c| c.is::<StagingDirectoryMissing>()));
        let diags = attached(&err).expect("the diagnostics stay attached");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, crate::codes::typst::SOURCE_ANALYSIS);
        assert_eq!(diags[0].notes.len(), 1);
    }
}
