//! Host adapters for the schema package's Issue boundary.
//!
//! The schema package evaluates in Typst. These adapters expose the one Rust implementation that
//! folds generated-value wrappers and renders the text a site author reads, so the package and
//! the compiler diagnostic cannot drift apart.

use typst::diag::{SourceDiagnostic, SourceResult, eco_vec};
use typst::foundations::{Array, Value, func};
use typst::syntax::Span;

use crate::issue::{self, Issue};

/// Fold generated-value wrappers into an explicit `origin` at the package boundary.
#[func]
pub(super) fn normalize_schema_issues(span: Span, issues: Array) -> SourceResult<Value> {
    let decoded = decode(&issues, span)?;
    let normalized = issue::normalize(&decoded);
    let mut encoded = Array::new();
    for issue in &normalized {
        encoded.push(issue.encode());
    }
    Ok(Value::Array(encoded))
}

/// Render Issues with the same sentences the compiler diagnostic uses.
#[func]
pub(super) fn format_schema_issues(span: Span, issues: Array) -> SourceResult<Value> {
    let decoded = decode(&issues, span)?;
    let rendered = issue::format(&decoded);
    Ok(Value::Str(rendered.as_str().into()))
}

fn decode(issues: &Array, span: Span) -> SourceResult<Vec<Issue>> {
    issues
        .iter()
        .map(|value| {
            Issue::decode(value).map_err(|message| eco_vec![SourceDiagnostic::error(span, message)])
        })
        .collect()
}
