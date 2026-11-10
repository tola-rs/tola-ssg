//! Report deduplication and failure detection.

use crate::{Diagnostic, DiagnosticError, Severity};

/// Merge repeated public issues from this one source snapshot without losing their context.
pub fn coalesce_reports(diagnostics: &mut Vec<Diagnostic>) {
    let mut merged: Vec<Diagnostic> = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics.drain(..) {
        let same = diagnostic.producer.as_ref().and_then(|_| {
            let location = diagnostic.location.as_ref()?;
            let range = location.range?;
            merged.iter_mut().find(|existing| {
                existing.producer == diagnostic.producer
                    && existing.severity == diagnostic.severity
                    && existing.message == diagnostic.message
                    && existing.location.as_ref().is_some_and(|at| {
                        let same_source = match (&existing.source, &diagnostic.source) {
                            (Some(left), Some(right)) => {
                                left.input_identity() == right.input_identity()
                            }
                            (None, None) => at.path == location.path,
                            _ => false,
                        };
                        same_source && at.range == Some(range)
                    })
            })
        });
        if let Some(existing) = same {
            merge_distinct(&mut existing.help, diagnostic.help);
            merge_distinct(&mut existing.related, diagnostic.related);
            merge_distinct(&mut existing.notes, diagnostic.notes);
            merge_distinct(&mut existing.trace, diagnostic.trace);
            merge_distinct(&mut existing.imported_by, diagnostic.imported_by);
        } else {
            merged.push(diagnostic);
        }
    }
    *diagnostics = merged;
}

fn merge_distinct<T: PartialEq>(retained: &mut Vec<T>, incoming: impl IntoIterator<Item = T>) {
    for item in incoming {
        if !retained.contains(&item) {
            retained.push(item);
        }
    }
}

/// Accumulated reports finish evaluation normally but prevent a successful host operation.
pub fn ensure_reports_succeed(diagnostics: &[Diagnostic]) -> Result<(), DiagnosticError> {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.producer.is_some() && diagnostic.severity == Severity::Error)
    {
        return Err(DiagnosticError::new(
            "a reported error prevents this operation",
            diagnostics.to_vec(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiagnosticProducer, DiagnosticSource, Help, SourcePosition, SourceRange};

    fn report(owner: &str, code: &str) -> Diagnostic {
        let mut diagnostic = Diagnostic::at_path(
            crate::codes::report::ISSUE,
            Severity::Warning,
            "content/a.typ",
            "one issue",
        );
        diagnostic.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 1,
                character: 3,
            },
            end: SourcePosition {
                line: 1,
                character: 5,
            },
        });
        diagnostic.producer = Some(DiagnosticProducer {
            name: Some(owner.into()),
            code: Some(code.into()),
        });
        diagnostic
    }

    #[test]
    fn reports_merge_by_source_revision() {
        let evidence = |path: &str, bytes: &[u8]| {
            DiagnosticSource::new(
                tola_typst::ReadLocator::ProvidedRoot(path.into()),
                tola_typst::ContentDigest::of(bytes),
            )
        };
        let mut first = report("command:catalog", "same-rule");
        first.source = Some(evidence("first.typ", b"same source text"));
        first.help.push(Help {
            message: "first correction".into(),
            location: None,
        });
        let mut repeated = first.clone();
        repeated.help = vec![Help {
            message: "second correction".into(),
            location: None,
        }];
        repeated.related.push(crate::RelatedInformation {
            message: "context".into(),
            location: None,
            source: None,
        });
        let mut other_source = first.clone();
        other_source.source = Some(evidence("second.typ", b"same source text"));
        let mut other_revision = first.clone();
        other_revision.source = Some(evidence("first.typ", b"changed source text"));
        let mut unkeyed = first.clone();
        unkeyed.source = None;
        let mut other_producer = first.clone();
        other_producer.producer.as_mut().unwrap().name = Some("command:feed".into());
        let mut other_rule = first.clone();
        other_rule.producer.as_mut().unwrap().code = Some("other-rule".into());
        let error = DiagnosticError::new(
            "report errors",
            vec![
                first,
                repeated.clone(),
                repeated,
                other_source,
                other_revision,
                unkeyed,
                other_producer,
                other_rule,
            ],
        );
        let diagnostics = error.diagnostics();
        assert_eq!(diagnostics.len(), 6);
        assert_eq!(diagnostics[0].help.len(), 2);
        assert_eq!(diagnostics[0].related.len(), 1);
        assert!(
            serde_json::to_value(&diagnostics[0])
                .unwrap()
                .get("source")
                .is_none()
        );
    }
}
