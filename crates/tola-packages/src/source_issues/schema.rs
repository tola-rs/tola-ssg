//! Schema argument and declaration diagnostics attributed to the author's call.

use tola_typst::diagnostic::{DiagnosticOrigin, NativeDiagnostic, producer_error};
use typst::comemo::Tracked;
use typst::diag::{At, SourceDiagnostic, SourceResult, eco_format};
use typst::ecow::{EcoVec, eco_vec};
use typst::engine::Engine;
use typst::foundations::{Context, Dict, Func, Str, Value, func};
use typst::syntax::{Span, VirtualRoot};

use super::location::source_origin;
use super::projection::{SCHEMA_WRAPPERS, wrapper_projection};

const SCHEMA_ARGUMENT: &str = "SchemaArgument";

#[func]
pub(crate) fn tola_report_schema_error(
    span: Span,
    /// The value supplied where a schema declaration was required.
    candidate: Value,
    /// The schema operation whose argument was invalid.
    owner: Str,
) -> SourceResult<Value> {
    let actual = candidate.ty().short_name();
    let mut err = producer_error(
        span,
        SCHEMA_ARGUMENT,
        &[("owner", owner.as_str()), ("actual", actual)],
        eco_format!("{owner}: expected a Typst type or schema declaration, found `{actual}`"),
        wrapper_projection(),
    );
    err.hint(if matches!(candidate, Value::Dict(_)) {
        "wrap the field dictionary with `schema(...)`"
    } else {
        "pass a Typst type such as `str` or a declaration such as `schema((title: str))`"
    });
    Err(eco_vec![err])
}

#[func(contextual)]
pub(crate) fn tola_parse_source(
    engine: &mut Engine,
    context: Tracked<Context>,
    span: Span,
    /// The complete source record whose metadata is being resolved.
    source: Dict,
    /// A declaration or a function choosing that source's declaration.
    schema: Value,
    /// The schema package's parser.
    parse: Func,
) -> SourceResult<Value> {
    let path = source
        .get("path")
        .at(span)?
        .clone()
        .cast::<Str>()
        .at(span)?;
    let origin = source_origin(engine, path.as_str()).at(span)?;
    let path = origin.owner.vpath().get_without_slash();
    let is_factory = matches!(schema, Value::Func(_));
    let decl = match schema {
        Value::Func(factory) => factory
            .call(engine, context, [Value::Dict(source.clone())])
            .map_err(|mut errs| {
                for err in errs.make_mut() {
                    err.hint(eco_format!("while choosing the schema for `{path}`"));
                }
                errs
            })?,
        other => other,
    };
    let meta = match source.get("meta").at(span)? {
        Value::None => Value::Dict(Dict::new()),
        val => val.clone(),
    };
    parse
        .call(engine, context, [meta, decl])
        .map_err(|errs| source_schema_errors(errs, path, is_factory))
}

fn source_schema_errors(
    mut errs: EcoVec<SourceDiagnostic>,
    path: &str,
    is_factory: bool,
) -> EcoVec<SourceDiagnostic> {
    for err in errs.make_mut() {
        if let Some(actual) = try_parse_actual(err) {
            err.message = if is_factory {
                eco_format!(
                    "schema function for `{path}` returned `{actual}`; expected a Typst type or schema declaration"
                )
            } else {
                eco_format!(
                    "parse-sources schema for `{path}` has type `{actual}`; expected a Typst type or schema declaration"
                )
            };
        } else {
            err.hint(eco_format!("while resolving the schema for `{path}`"));
        }
    }
    errs
}

fn try_parse_actual(err: &SourceDiagnostic) -> Option<String> {
    let native = NativeDiagnostic::from(err.clone());
    let DiagnosticOrigin::Producer(origin) = native.origin() else {
        return None;
    };
    if origin.name() != SCHEMA_ARGUMENT
        || !origin
            .fields()
            .iter()
            .any(|(k, v)| k == "owner" && v == "try-parse")
        || has_caller_frame(err)
    {
        return None;
    }
    origin
        .fields()
        .iter()
        .find_map(|(k, v)| (k == "actual").then(|| v.to_string()))
}

fn has_caller_frame(err: &SourceDiagnostic) -> bool {
    err.trace.iter().any(|t| {
        t.span.id().is_some_and(|id| match id.root() {
            VirtualRoot::Project => true,
            VirtualRoot::Package(pkg) => !SCHEMA_WRAPPERS.into_iter().any(|w| w.spec() == *pkg),
        })
    })
}
