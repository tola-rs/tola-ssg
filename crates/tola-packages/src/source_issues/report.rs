//! Diagnostics for source metadata issues, located at the source they belong to.

use typst::World;
use typst::diag::{At, SourceDiagnostic, SourceResult, bail, eco_format};
use typst::ecow::{EcoVec, eco_vec};
use typst::engine::Engine;
use typst::foundations::{Array, Value, func};
use typst::syntax::{FileId, Span};

use super::location::{IssueOrigin, source_origin};
use super::projection::wrapper_projection;
use crate::issue::{Issue, Region, describe, issue_path};

/// Report every issue of one `parse-sources` call against its source metadata.
#[func]
pub(crate) fn tola_report_source_issues(
    engine: &mut Engine,
    span: Span,
    /// One `(path, issue)` record per schema Issue, in source order.
    issues: Array,
) -> SourceResult<Value> {
    let mut diagnostics = EcoVec::new();
    for record in issues.iter() {
        diagnostics.extend(source_record(engine, span, record)?);
    }
    if diagnostics.is_empty() {
        return Ok(Value::None);
    }
    Err(diagnostics)
}

/// Every diagnostic one `(path, issue)` record produces. A raw wrapper can stand for several
/// causes, so normalization may return more than one diagnostic for one record.
fn source_record(
    engine: &Engine<'_>,
    span: Span,
    record: &Value,
) -> SourceResult<Vec<SourceDiagnostic>> {
    let described = "parse-sources issues are `(path, issue)` records";
    let Value::Dict(record) = record else {
        bail!(span, "{described}");
    };
    let Ok(Value::Str(path)) = record.get("path") else {
        bail!(span, "{described}");
    };
    let decoded = Issue::decode(record.get("issue").at(span)?)
        .map_err(|message| eco_vec![SourceDiagnostic::error(span, message)])?;
    let mut diagnostics = Vec::new();
    for problem in crate::issue::normalize(&[decoded]) {
        diagnostics.push(source_issue(engine, span, path.as_str(), &problem)?);
    }
    Ok(diagnostics)
}

fn source_issue(
    engine: &Engine<'_>,
    span: Span,
    path: &str,
    problem: &Issue,
) -> SourceResult<SourceDiagnostic> {
    let (file, declaration) = source_origin(engine, path).at(span)?;
    let site_path = file.vpath().get_without_slash().to_owned();
    let file = FileId::new(file);
    let source = engine.world.source(file).at(span)?;
    let origin = IssueOrigin {
        source: &source,
        declaration,
    };
    let message = match &problem.origin {
        Some(problem_origin) => eco_format!(
            "`{site_path}`: {}",
            problem.sentence(&problem.path, problem_origin.kind, &problem_origin.path)
        ),
        None => eco_format!(
            "`{site_path}`: `{}`: {}",
            issue_path(&problem.path),
            problem.message
        ),
    };
    let range = match &problem.origin {
        Some(_) => origin.marker_range(),
        None => origin.locate(&problem.path),
    };
    let mut diagnostic = tola_typst::diagnostic::producer_error_at(
        source.id(),
        range,
        crate::diagnostic::SOURCE_ISSUE_ORIGIN,
        &[],
        message,
        wrapper_projection(),
    );
    append_hints(problem, &origin, &mut diagnostic);
    Ok(diagnostic)
}

fn append_hints(problem: &Issue, origin: &IssueOrigin<'_>, diagnostic: &mut SourceDiagnostic) {
    for note in &problem.notes {
        diagnostic.hint(note.as_str());
    }
    let region = match &problem.origin {
        Some(problem_origin) => Region::generated(problem.path.clone(), problem_origin.kind),
        None => Region::root(),
    };
    for child in &problem.children {
        append_child(child, &[], &region, origin, diagnostic);
    }
}

fn append_child(
    issue: &Issue,
    path: &[Value],
    region: &Region,
    origin: &IssueOrigin<'_>,
    diagnostic: &mut SourceDiagnostic,
) {
    if let Some(description) = describe(issue, path, region) {
        diagnostic.spanned_hint(
            issue.sentence(
                &description.field,
                description.kind,
                &description.origin_path,
            ),
            origin.span(origin.marker_range()),
        );
        for note in &issue.notes {
            diagnostic.hint(note.as_str());
        }
        for child in &issue.children {
            append_child(child, &[], &description.child, origin, diagnostic);
        }
    } else {
        let full = [path, &issue.path].concat();
        let range = if region.is_generated() {
            origin.marker_range()
        } else {
            origin.locate(&full)
        };
        diagnostic.spanned_hint(
            eco_format!("{}: {}", region.position(&full), issue.message),
            origin.span(range),
        );
        for note in &issue.notes {
            diagnostic.hint(note.as_str());
        }
        for child in &issue.children {
            append_child(child, &full, region, origin, diagnostic);
        }
    }
}
