//! Host adapters for `@tola/schema` Issue normalization and rendering.

use typst::diag::{SourceDiagnostic, SourceResult, eco_vec};
use typst::foundations::{Array, Value, func};
use typst::syntax::Span;

use crate::issue::{self, Issue};

#[func]
pub(super) fn normalize_schema_issues(span: Span, issues: Array) -> SourceResult<Value> {
    let decoded = decode(&issues, span)?;
    let encoded: Array = issue::normalize(&decoded)
        .iter()
        .map(Issue::encode)
        .collect();
    Ok(Value::Array(encoded))
}

#[func]
pub(super) fn format_schema_issues(span: Span, issues: Array) -> SourceResult<Value> {
    let decoded = decode(&issues, span)?;
    Ok(Value::Str(issue::format(&decoded).as_str().into()))
}

fn decode(issues: &Array, span: Span) -> SourceResult<Vec<Issue>> {
    issues
        .iter()
        .map(|val| Issue::decode(val).map_err(|msg| eco_vec![SourceDiagnostic::error(span, msg)]))
        .collect()
}

/// Normalized Issue located relative to the supplied value.
#[derive(Debug, Clone)]
pub struct ProjectedIssue {
    pub path: Vec<Value>,
    pub generated: bool,
    pub message: String,
    pub notes: Vec<String>,
}

/// Decodes and normalizes schema Issues with `format-issues` provenance and wording.
pub fn project_issues(issues: &Array) -> Result<Vec<ProjectedIssue>, String> {
    let decoded = issues
        .iter()
        .map(|val| Issue::decode(val).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for issue in issue::normalize(&decoded) {
        project_one(&issue, &[], &issue::Region::root(), None, &mut out);
    }
    Ok(out)
}

fn project_one(
    issue: &Issue,
    prefix: &[Value],
    region: &issue::Region,
    gen_field: Option<&[Value]>,
    out: &mut Vec<ProjectedIssue>,
) {
    let notes = issue.notes.iter().map(ToString::to_string).collect();
    let (path, generated, message, child_prefix, child_region, child_field) =
        match issue::describe(issue, prefix, region) {
            Some(desc) => (
                desc.field.clone(),
                true,
                issue
                    .sentence(&desc.field, desc.kind, &desc.origin_path)
                    .to_string(),
                Vec::new(),
                desc.child,
                Some(desc.field),
            ),
            None => {
                let full = [prefix, &issue.path].concat();
                (
                    gen_field.map_or_else(|| full.clone(), |f| f.to_vec()),
                    gen_field.is_some(),
                    format!("{}: {}", region.position(&full), issue.message),
                    full,
                    region.clone(),
                    gen_field.map(|f| f.to_vec()),
                )
            }
        };
    out.push(ProjectedIssue {
        path,
        generated,
        message,
        notes,
    });
    for child in &issue.children {
        project_one(
            child,
            &child_prefix,
            &child_region,
            child_field.as_deref(),
            out,
        );
    }
}
