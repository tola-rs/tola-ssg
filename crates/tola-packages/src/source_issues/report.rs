//! Source metadata issue diagnostics attributed to the declaring source.

use typst::World;
use typst::diag::{At, SourceDiagnostic, SourceResult, bail, eco_format};
use typst::ecow::{EcoVec, eco_vec};
use typst::engine::Engine;
use typst::foundations::{Array, Value, func};
use typst::syntax::{FileId, Span};

use super::location::{IssueOrigin, source_origin};
use super::projection::wrapper_projection;
use crate::issue::{Issue, Region, describe, issue_path};

#[func]
pub(crate) fn tola_report_source_issues(
    engine: &mut Engine,
    span: Span,
    /// One `(path, issue)` record per schema Issue, in source order.
    issues: Array,
) -> SourceResult<Value> {
    let mut diags = EcoVec::new();
    for rec in issues.iter() {
        diags.extend(source_record(engine, span, rec)?);
    }
    if diags.is_empty() {
        return Ok(Value::None);
    }
    Err(diags)
}

fn source_record(
    engine: &Engine<'_>,
    span: Span,
    rec: &Value,
) -> SourceResult<Vec<SourceDiagnostic>> {
    let shape = "parse-sources issues are `(path, issue)` records";
    let Value::Dict(dict) = rec else {
        bail!(span, "{shape}");
    };
    let Some(path) = crate::dict_str(dict, "path") else {
        bail!(span, "{shape}");
    };
    let decoded = Issue::decode(dict.get("issue").at(span)?)
        .map_err(|msg| eco_vec![SourceDiagnostic::error(span, msg)])?;
    crate::issue::normalize(&[decoded])
        .iter()
        .map(|issue| source_issue(engine, span, path, issue))
        .collect()
}

fn source_issue(
    engine: &Engine<'_>,
    span: Span,
    path: &str,
    issue: &Issue,
) -> SourceResult<SourceDiagnostic> {
    let src_origin = source_origin(engine, path).at(span)?;
    let site_path = src_origin.owner.vpath().get_without_slash();
    let src = engine.world.source(FileId::new(src_origin.file)).at(span)?;
    let origin = IssueOrigin {
        source: &src,
        declaration: src_origin.range,
    };
    let (msg, range) = match &issue.origin {
        Some(o) => (
            eco_format!(
                "`{site_path}`: {}",
                issue.sentence(&issue.path, o.kind, &o.path)
            ),
            origin.marker_range(),
        ),
        None => (
            eco_format!(
                "`{site_path}`: `{}`: {}",
                issue_path(&issue.path),
                issue.message
            ),
            origin.locate(&issue.path),
        ),
    };
    let mut diag = tola_typst::diagnostic::producer_error_at(
        src.id(),
        range,
        crate::diagnostic::SOURCE_ISSUE_ORIGIN,
        &[],
        msg,
        wrapper_projection(),
    );
    append_hints(issue, &origin, &mut diag);
    Ok(diag)
}

fn append_hints(issue: &Issue, origin: &IssueOrigin<'_>, diag: &mut SourceDiagnostic) {
    for note in &issue.notes {
        diag.hint(note.as_str());
    }
    let region = match &issue.origin {
        Some(o) => Region::generated(issue.path.clone(), o.kind),
        None => Region::root(),
    };
    for child in &issue.children {
        append_child(child, &[], &region, origin, diag);
    }
}

fn append_child(
    issue: &Issue,
    path: &[Value],
    region: &Region,
    origin: &IssueOrigin<'_>,
    diag: &mut SourceDiagnostic,
) {
    let (hint, range, child_path, child_region) = match describe(issue, path, region) {
        Some(desc) => (
            issue.sentence(&desc.field, desc.kind, &desc.origin_path),
            origin.marker_range(),
            Vec::new(),
            desc.child,
        ),
        None => {
            let full = [path, &issue.path].concat();
            let range = if region.is_generated() {
                origin.marker_range()
            } else {
                origin.locate(&full)
            };
            (
                eco_format!("{}: {}", region.position(&full), issue.message),
                range,
                full,
                region.clone(),
            )
        }
    };
    diag.spanned_hint(hint, origin.span(range));
    for note in &issue.notes {
        diag.hint(note.as_str());
    }
    for child in &issue.children {
        append_child(child, &child_path, &child_region, origin, diag);
    }
}
