//! Schema argument and declaration diagnostics, located at the author's call.

use typst::comemo::Tracked;
use typst::diag::{At, SourceDiagnostic, SourceResult, eco_format};
use typst::ecow::{EcoVec, eco_vec};
use typst::engine::Engine;
use typst::foundations::{Context, Dict, Func, Value, func};
use typst::syntax::Span;

use super::location::source_origin;
use super::projection::{SCHEMA_WRAPPERS, wrapper_projection};

const SCHEMA_ARGUMENT: &str = "SchemaArgument";
#[func]
pub(crate) fn tola_report_schema_error(
    span: Span,
    /// The value supplied where a schema declaration was required.
    candidate: Value,
    /// The schema operation whose argument was invalid.
    owner: typst::foundations::Str,
) -> SourceResult<Value> {
    let actual = candidate.ty().short_name();
    let mut error = tola_typst::diagnostic::producer_error(
        span,
        SCHEMA_ARGUMENT,
        &[("owner", owner.as_str()), ("actual", actual)],
        eco_format!("{owner}: expected a Typst type or schema declaration, found `{actual}`"),
        wrapper_projection(),
    );
    error.hint(if matches!(candidate, Value::Dict(_)) {
        "wrap the field dictionary with `schema(...)`"
    } else {
        "pass a Typst type such as `str` or a declaration such as `schema((title: str))`"
    });
    Err(eco_vec![error])
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
        .cast::<typst::foundations::Str>()
        .at(span)?;
    let (file, _) = source_origin(engine, path.as_str()).at(span)?;
    let path = file.vpath().get_without_slash();
    let is_factory = matches!(schema, Value::Func(_));
    let declaration = if let Value::Func(factory) = schema {
        factory
            .call(engine, context, [Value::Dict(source.clone())])
            .map_err(|mut errors| {
                for error in errors.make_mut() {
                    error.hint(eco_format!("while choosing the schema for `{path}`"));
                }
                errors
            })?
    } else {
        schema
    };
    let metadata = match source.get("meta").at(span)? {
        Value::None => Value::Dict(Dict::new()),
        value => value.clone(),
    };
    parse
        .call(engine, context, [metadata, declaration])
        .map_err(|errors| source_schema_errors(errors, path, is_factory))
}

fn source_schema_errors(
    mut errors: EcoVec<SourceDiagnostic>,
    path: &str,
    is_factory: bool,
) -> EcoVec<SourceDiagnostic> {
    for error in errors.make_mut() {
        let native = tola_typst::diagnostic::NativeDiagnostic::from(error.clone());
        if let tola_typst::diagnostic::DiagnosticOrigin::Producer(origin) = native.origin()
            && origin.name() == SCHEMA_ARGUMENT
            && origin
                .fields()
                .iter()
                .any(|(key, value)| key == "owner" && value == "try-parse")
            && !error.trace.iter().any(|trace| {
                trace.span.id().is_some_and(|id| match id.root() {
                    typst::syntax::VirtualRoot::Project => true,
                    typst::syntax::VirtualRoot::Package(package) => !SCHEMA_WRAPPERS
                        .into_iter()
                        .any(|owned| owned.spec() == *package),
                })
            })
            && let Some((_, actual)) = origin.fields().iter().find(|(key, _)| key == "actual")
        {
            error.message = if is_factory {
                eco_format!(
                    "schema function for `{path}` returned `{actual}`; expected a Typst type or schema declaration"
                )
            } else {
                eco_format!(
                    "parse-sources schema for `{path}` has type `{actual}`; expected a Typst type or schema declaration"
                )
            };
        } else {
            error.hint(eco_format!("while resolving the schema for `{path}`"));
        }
    }
    errors
}
