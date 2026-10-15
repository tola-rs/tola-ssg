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

/// One normalized Issue located relative to the supplied value.
#[derive(Debug, Clone)]
pub struct ProjectedIssue {
    /// Path in the original user-supplied value.
    pub path: Vec<Value>,
    /// Whether the rejected value came from the schema, rather than the user.
    pub generated: bool,
    /// Text rendered with the schema package's own Issue wording.
    pub message: String,
    /// Additional notes carried by this Issue.
    pub notes: Vec<String>,
}

/// Decode and normalize schema Issues with the same provenance and wording as `format-issues`.
pub fn project_issues(issues: &Array) -> Result<Vec<ProjectedIssue>, String> {
    let decoded = issues
        .iter()
        .map(|value| Issue::decode(value).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut projected = Vec::new();
    for issue in issue::normalize(&decoded) {
        project_one(&issue, &[], &issue::Region::root(), None, &mut projected);
    }
    Ok(projected)
}

fn project_one(
    problem: &Issue,
    prefix: &[Value],
    region: &issue::Region,
    generated_field: Option<&[Value]>,
    output: &mut Vec<ProjectedIssue>,
) {
    let full = [prefix, &problem.path].concat();
    let notes = problem.notes.iter().map(ToString::to_string).collect();
    if let Some(description) = issue::describe(problem, prefix, region) {
        let message = problem
            .sentence(
                &description.field,
                description.kind,
                &description.origin_path,
            )
            .to_string();
        output.push(ProjectedIssue {
            path: description.field.clone(),
            generated: true,
            message,
            notes,
        });
        for child in &problem.children {
            project_one(
                child,
                &[],
                &description.child,
                Some(&description.field),
                output,
            );
        }
    } else {
        output.push(ProjectedIssue {
            path: generated_field.map_or_else(|| full.clone(), |field| field.to_vec()),
            generated: generated_field.is_some(),
            message: format!("{}: {}", region.position(&full), problem.message),
            notes,
        });
        for child in &problem.children {
            project_one(child, &full, region, generated_field, output);
        }
    }
}
