//! Reporting a declaration's issues as diagnostics about the sources that raised them.

use typst::diag::{At, SourceDiagnostic, SourceResult, bail, eco_format};
use typst::ecow::{EcoString, EcoVec};
use typst::engine::Engine;
use typst::foundations::{Array, Value, func};
use typst::syntax::{DiagSpan, FileId, RootedPath, Span};

/// Report every issue of one `parse-sources` call as its own diagnostic.
///
/// An issue names the source it was found in, and its diagnostic is anchored at that source's
/// `<tola-meta>` declaration, which is the line its author has to change. The message is already
/// rendered against the value's own paths by `format-issues`, so it is carried unchanged.
#[func]
pub(super) fn tola_report_source_issues(
    engine: &mut Engine,
    span: Span,
    /// One `(path, message)` record per issue, in the order it was found.
    issues: Array,
) -> SourceResult<Value> {
    let mut diagnostics = EcoVec::new();
    for issue in issues.iter() {
        diagnostics.push(source_issue(engine, span, issue)?);
    }
    if diagnostics.is_empty() {
        return Ok(Value::None);
    }
    Err(diagnostics)
}

/// One issue as a diagnostic about the declaration of the source that raised it.
fn source_issue(engine: &Engine<'_>, span: Span, issue: &Value) -> SourceResult<SourceDiagnostic> {
    let Value::Dict(issue) = issue else {
        bail!(
            span,
            "parse-sources issues are `(path, message)` records; found {}",
            issue.ty().short_name()
        )
    };
    let described = "parse-sources issues are `(path, message)` records";
    let Ok(Value::Str(path)) = issue.get("path") else {
        bail!(span, "{described}");
    };
    let Ok(Value::Str(message)) = issue.get("message") else {
        bail!(span, "{described}");
    };
    // Both the file and the declaration range come from the source's one origin, so a diagnostic
    // can never name one source's file with another source's declaration. The message names that
    // file the way the diagnostic's own location does — below the site root — while `path` is the
    // source's identity below `build.content-dir`, which only locates the origin.
    let (file, range) = source_origin(engine, path.as_str()).at(span)?;
    let site_path = file.vpath().get_without_slash().to_owned();
    let file = FileId::new(file);
    Ok(SourceDiagnostic::error(
        DiagSpan::from_range(file, range),
        eco_format!("`{site_path}`: {}", message.as_str()),
    ))
}

/// Where `path` lives and declares its metadata, as the file it is addressed by and the byte range
/// of its `<tola-meta>` declaration.
///
/// A source that declares no metadata has no range, and the file's start answers for it: the
/// diagnostic still names the file and the field, and there is no declaration to point at. A path
/// no origin covers is not one of this compilation's sources, which is a programming error rather
/// than a position to guess.
fn source_origin(
    engine: &Engine<'_>,
    path: &str,
) -> Result<(RootedPath, std::ops::Range<usize>), EcoString> {
    let Some(Value::Dict(origin)) =
        super::library::source_origins(engine).and_then(|origins| origins.get(path).ok())
    else {
        return Err(eco_format!("`{path}` is not a source of this compilation"));
    };
    let file = origin
        .get("file")
        .map_err(|_| eco_format!("`{path}` has an origin without a file"))?
        .clone()
        .cast::<RootedPath>()
        .map_err(|_| eco_format!("`{path}` has an origin whose file is not a path"))?;
    let range = match origin.get("range") {
        Ok(Value::None) => 0..0,
        Ok(Value::Array(range)) => declaration_bounds(path, range)?,
        _ => {
            return Err(eco_format!(
                "`{path}` has an origin whose range is not an array or `none`"
            ));
        }
    };
    Ok((file, range))
}

/// The byte range of one source's `<tola-meta>` declaration, as its two integer bounds.
fn declaration_bounds(path: &str, range: &Array) -> Result<std::ops::Range<usize>, EcoString> {
    let mut bounds = range.iter();
    let bound = |value: Option<&Value>, which: &str| -> Result<i64, EcoString> {
        match value {
            Some(Value::Int(bound)) => Ok(*bound),
            _ => Err(eco_format!(
                "`{path}` declares metadata at a range without a {which}"
            )),
        }
    };
    let start = bound(bounds.next(), "start")?;
    let end = bound(bounds.next(), "end")?;
    if start < 0 || end < start {
        return Err(eco_format!(
            "`{path}` declares metadata at a range that runs backwards: {start}..{end}"
        ));
    }
    Ok(start as usize..end as usize)
}
