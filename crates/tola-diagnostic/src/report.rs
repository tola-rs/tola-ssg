//! Report deduplication and failure detection.

use crate::{Diagnostic, DiagnosticError, Severity};

pub fn coalesce_reports(diags: &mut Vec<Diagnostic>) {
    let mut out: Vec<Diagnostic> = Vec::with_capacity(diags.len());
    for diag in diags.drain(..) {
        let same = diag.producer.as_ref().and_then(|_| {
            let loc = diag.location.as_ref()?;
            let range = loc.range?;
            out.iter_mut().find(|cur| {
                cur.producer == diag.producer
                    && cur.severity == diag.severity
                    && cur.message == diag.message
                    && cur.location.as_ref().is_some_and(|at| {
                        let same_src = match (&cur.source, &diag.source) {
                            (Some(l), Some(r)) => l.input_identity() == r.input_identity(),
                            (None, None) => at.path == loc.path,
                            _ => false,
                        };
                        same_src && at.range == Some(range)
                    })
            })
        });
        if let Some(cur) = same {
            merge_distinct(&mut cur.help, diag.help);
            merge_distinct(&mut cur.related, diag.related);
            merge_distinct(&mut cur.notes, diag.notes);
            merge_distinct(&mut cur.trace, diag.trace);
            merge_distinct(&mut cur.imported_by, diag.imported_by);
        } else {
            out.push(diag);
        }
    }
    *diags = out;
}

fn merge_distinct<T: PartialEq>(dst: &mut Vec<T>, src: impl IntoIterator<Item = T>) {
    for item in src {
        if !dst.contains(&item) {
            dst.push(item);
        }
    }
}

pub fn ensure_reports_succeed(diags: &[Diagnostic]) -> Result<(), DiagnosticError> {
    if diags
        .iter()
        .any(|d| d.producer.is_some() && d.severity == Severity::Error)
    {
        return Err(DiagnosticError::new(
            "a reported error prevents this operation",
            diags.to_vec(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiagnosticProducer, DiagnosticSource, Help, SourcePosition, SourceRange};

    fn report(owner: &str, code: &str) -> Diagnostic {
        let mut diag = Diagnostic::at_path(
            crate::codes::report::ISSUE,
            Severity::Warning,
            "content/a.typ",
            "one issue",
        );
        diag.location.as_mut().unwrap().range = Some(SourceRange {
            start: SourcePosition {
                line: 1,
                character: 3,
            },
            end: SourcePosition {
                line: 1,
                character: 5,
            },
        });
        diag.producer = Some(DiagnosticProducer {
            name: Some(owner.into()),
            code: Some(code.into()),
        });
        diag
    }

    #[test]
    fn merge_by_source_rev() {
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
        let mut other_src = first.clone();
        other_src.source = Some(evidence("second.typ", b"same source text"));
        let mut other_rev = first.clone();
        other_rev.source = Some(evidence("first.typ", b"changed source text"));
        let mut unkeyed = first.clone();
        unkeyed.source = None;
        let mut other_prod = first.clone();
        other_prod.producer.as_mut().unwrap().name = Some("command:feed".into());
        let mut other_rule = first.clone();
        other_rule.producer.as_mut().unwrap().code = Some("other-rule".into());
        let err = DiagnosticError::new(
            "report errors",
            vec![
                first,
                repeated.clone(),
                repeated,
                other_src,
                other_rev,
                unkeyed,
                other_prod,
                other_rule,
            ],
        );
        let diags = err.diagnostics();
        assert_eq!(diags.len(), 6);
        assert_eq!(diags[0].help.len(), 2);
        assert_eq!(diags[0].related.len(), 1);
        assert!(
            serde_json::to_value(&diags[0])
                .unwrap()
                .get("source")
                .is_none()
        );
    }
}
