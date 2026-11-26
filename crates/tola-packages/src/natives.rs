//! Native functions imported by builtin packages from `@tola/host`.

use std::sync::LazyLock;

use typst::World;
use typst::comemo::Tracked;
use typst::diag::{At, SourceDiagnostic, SourceResult, bail};
use typst::ecow::eco_vec;
use typst::engine::Engine;
use typst::foundations::{Array, Context, Dict, IntoValue, Str, Value, func};
use typst::introspection::{DocumentIntrospection, PathIntrospection, here};
use typst::syntax::{FileId, RootedPath, Span, Spanned, VirtualPath, VirtualRoot};

use crate::TolaPackage;
use crate::protocol::{CAPABILITY_OBSERVATION_PATH, SOURCE_OBSERVATION_DIRECTORY};

fn address_err(
    span: Span,
    func: &str,
    msg: impl std::fmt::Display,
) -> typst::ecow::EcoVec<SourceDiagnostic> {
    eco_vec![crate::diagnostic::package_error(
        span,
        TolaPackage::Address,
        format!("`{func}`: {msg}"),
    )]
}

#[func]
pub(super) fn project_path(
    span: Span,
    /// A path relative to the site's root directory.
    path: Str,
) -> SourceResult<RootedPath> {
    let vpath = VirtualPath::new(path.as_str())
        .map_err(|err| err.to_string())
        .at(span)?;
    Ok(RootedPath::new(VirtualRoot::Project, vpath))
}

static SOURCES_CAPABILITY_FILE_ID: LazyLock<FileId> = LazyLock::new(|| {
    FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Source.spec()),
        VirtualPath::new(CAPABILITY_OBSERVATION_PATH)
            .expect("capability observation path is virtualizable"),
    ))
});

#[func]
pub(super) fn tola_sources(engine: &mut Engine, span: Span) -> SourceResult<Array> {
    // Reading this observation records the sources dependency for retained source-analysis entries.
    engine.world.source(*SOURCES_CAPABILITY_FILE_ID).at(span)?;
    let Some(srcs) = super::library::source_records(engine) else {
        bail!(span, "Tola could not read the site's content sources");
    };
    Ok(srcs.clone())
}

#[func(contextual)]
pub(super) fn tola_current(
    engine: &mut Engine,
    context: Tracked<Context>,
    span: Span,
) -> SourceResult<Dict> {
    let loc = here(context).at(span)?;
    let doc = engine.introspect(DocumentIntrospection(loc, span));
    let path = engine.introspect(PathIntrospection(loc, span));
    let (Some(doc), Some(path)) = (doc, path) else {
        bail!(
            span,
            "`current-document()` is called outside a `document(…)` body";
            hint: "call it in `context` inside a `document(…)` body"
        );
    };
    let out = tola_address::OutputPath::parse(path.get_with_slash().trim_start_matches('/'))
        .map_err(|err| err.to_string())
        .at(span)?;
    let route = tola_address::route_for_output(&out);
    Ok(crate::dict([
        (CURRENT_DOCUMENT_FIELDS[0], out.as_str().into_value()),
        (CURRENT_DOCUMENT_FIELDS[1], route.as_str().into_value()),
        (CURRENT_DOCUMENT_FIELDS[2], doc.into_value()),
    ]))
}

/// Fields published by `current-document()`, in insertion order.
pub(crate) const CURRENT_DOCUMENT_FIELDS: [&str; 3] = ["output", "route", "location"];

#[func]
pub(super) fn tola_source(engine: &mut Engine, span: Span) -> SourceResult<Dict> {
    let Some(id) = span.id() else {
        bail!(
            span,
            "`current-source()` is not written in a file";
            hint: "call it in the content source and pass the value on"
        );
    };
    match id.root() {
        VirtualRoot::Project => {}
        VirtualRoot::Package(pkg) => bail!(
            span,
            "`current-source()` is written in the package `{pkg}`";
            hint: "call it in the site's own content source and pass the value on"
        ),
    }
    let rel = id.vpath().get_with_slash();
    let obs = FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Source.spec()),
        VirtualPath::new(format!("{SOURCE_OBSERVATION_DIRECTORY}{rel}"))
            .expect("source observation extends a validated virtual path"),
    ));
    engine.world.source(obs).at(span)?;
    let Some(by_file) = super::library::source_records_by_file(engine) else {
        bail!(span, "Tola could not read the site's content sources");
    };
    let Ok(record) = by_file.get(rel).cloned() else {
        bail!(
            span,
            "`{rel}` is not a content source";
            hint: "call `current-source()` in a source under `build.content-dir`"
        );
    };
    record.cast::<Dict>().at(span)
}

fn naming_rules(
    mode: Spanned<Str>,
    case: Spanned<Str>,
    separator: Spanned<Str>,
    lang: &Str,
) -> SourceResult<tola_slugify::NamingRules> {
    use tola_slugify::{HanPronunciations, NamingRules, SlugCase, SlugMode, SlugSeparator};

    let mode = match mode.v.as_str() {
        "unicode" => SlugMode::Unicode,
        "ascii" => SlugMode::Ascii,
        other => bail!(
            mode.span,
            "`mode` must be `unicode` or `ascii`, not `{other}`"
        ),
    };
    let case = match case.v.as_str() {
        "lower" => SlugCase::Lower,
        "upper" => SlugCase::Upper,
        "capitalize" => SlugCase::Capitalize,
        "preserve" => SlugCase::Preserve,
        other => bail!(
            case.span,
            "`case` must be `lower`, `upper`, `capitalize`, or `preserve`, not `{other}`"
        ),
    };
    let separator = match separator.v.as_str() {
        "-" => SlugSeparator::Dash,
        "_" => SlugSeparator::Underscore,
        other => bail!(
            separator.span,
            "`separator` must be `-` or `_`, not `{other}`"
        ),
    };
    Ok(NamingRules {
        mode,
        case,
        separator,
        pronunciations: HanPronunciations::for_language(lang.as_str()),
    })
}

#[func]
pub(super) fn tola_slug(
    /// The text to turn into a slug.
    text: Spanned<Str>,
    /// If omitted, keep Unicode letters and numbers. Set `ascii` to transliterate them.
    #[named]
    #[default(Spanned::detached("unicode".into()))]
    mode: Spanned<Str>,
    /// Letter case. If omitted, use `lower`. Other choices are `upper`, `capitalize`, and `preserve`.
    #[named]
    #[default(Spanned::detached("lower".into()))]
    case: Spanned<Str>,
    /// Word separator. If omitted, use `-`. Pass `_` to use underscores.
    #[named]
    #[default(Spanned::detached("-".into()))]
    separator: Spanned<Str>,
    /// The tag naming the Han pronunciations `ascii` mode takes, such as `"zh"` or `"ja"`,
    /// defaulting to Chinese. A primary `ja` subtag selects Japanese, ignoring case (`"JA-JP"`
    /// also matches); every other tag selects Chinese.
    #[named]
    #[default("zh".into())]
    language: Str,
) -> SourceResult<Str> {
    let rules = naming_rules(mode, case, separator, &language)?;
    let slug = tola_slugify::slugify_segment(text.v.as_str(), rules)
        .ok_or("nothing in this text can name a segment")
        .at(text.span)?;
    Ok(slug.into())
}

#[func]
pub(super) fn tola_join_route(span: Span, segments: Array) -> SourceResult<Str> {
    let mut route = String::from("/");
    for (idx, val) in segments.iter().enumerate() {
        let Value::Str(seg) = val else {
            bail!(
                span,
                "route segment {} must be a string, found {}",
                idx + 1,
                val.ty().short_name()
            );
        };
        if seg.as_str().contains(['/', '\\']) {
            bail!(
                span,
                "route segment {} (`{seg}`) names more than one directory",
                idx + 1;
                hint: "pass one segment per element"
            );
        }
        if idx > 0 {
            route.push('/');
        }
        route.push_str(seg);
    }
    if !segments.is_empty() {
        route.push('/');
    }
    let route = tola_address::UrlPath::from_decoded(&route)
        .map_err(|err| err.to_string())
        .at(span)?;
    Ok(route.as_str().into())
}

#[func]
pub(super) fn tola_output_of(route: Spanned<Str>) -> SourceResult<Str> {
    let route = tola_address::UrlPath::from_decoded(route.v.as_str()).map_err(|err| {
        address_err(
            route.span,
            "output-of",
            format!("`route` is invalid: {err}"),
        )
    })?;
    Ok(tola_address::OutputPath::from_route(&route).as_str().into())
}

#[func]
pub(super) fn tola_route_of(output: Spanned<Str>) -> SourceResult<Str> {
    let out = tola_address::OutputPath::parse(output.v.as_str())
        .map_err(|err| err.to_string())
        .at(output.span)?;
    Ok(tola_address::route_for_output(&out).as_str().into())
}

#[func]
pub(super) fn tola_output_url(
    span: Span,
    output: Spanned<Str>,
    #[named]
    #[default("/".into())]
    base_path: Str,
    #[named]
    #[default]
    origin: Option<Str>,
) -> SourceResult<Str> {
    let out = tola_address::OutputPath::parse(output.v.as_str()).map_err(|err| {
        address_err(
            output.span,
            "output-url",
            format!("`output` is invalid: {err}"),
        )
    })?;
    let mount = parse_mount(base_path.as_str(), span)?;
    let origin = origin
        .as_deref()
        .map(|raw| parse_origin(raw, span))
        .transpose()?;
    let route = tola_address::route_for_output(&out);
    Ok(tola_address::browser_url(&route, &mount, origin.as_ref()).into())
}

fn parse_mount(base_path: &str, span: Span) -> SourceResult<tola_address::SiteUrlMount> {
    use tola_address::SiteUrlMountError;
    tola_address::SiteUrlMount::from_base_path(base_path).map_err(|err| {
        let reason = match err {
            SiteUrlMountError::InvalidBasePath(r) => format!("is not a valid URL path: {r}"),
            SiteUrlMountError::MissingTrailingSlash => "must end with `/`".to_owned(),
            SiteUrlMountError::ReservedRoot => {
                "must stay outside `_tola`, the namespace Tola reserves".to_owned()
            }
        };
        address_err(span, "output-url", format!("`base-path` {reason}"))
    })
}

fn parse_origin(raw: &str, span: Span) -> SourceResult<tola_address::SiteOrigin> {
    let origin = tola_address::SiteOrigin::parse(raw)
        .map_err(|err| address_err(span, "output-url", format!("`origin` {err}")))?;
    if origin.base_path().is_some() {
        return Err(address_err(
            span,
            "output-url",
            "`origin` must name only an http/https origin; pass the deployment path with `base-path`",
        ));
    }
    Ok(origin)
}

#[func]
pub(super) fn tola_decode_url_path(url_path: Spanned<Str>) -> SourceResult<Str> {
    let path = tola_address::UrlPath::parse(url_path.v.as_str())
        .map_err(|err| err.to_string())
        .at(url_path.span)?;
    Ok(path.as_str().into())
}

#[func]
pub(super) fn tola_parse_url_path(url_path: Str) -> Dict {
    schema_conversion(
        tola_address::UrlPath::parse(url_path.as_str()).map(|p| p.as_str().to_owned()),
        "schema.url-path",
    )
}

#[func]
pub(super) fn tola_parse_date(value: Value) -> Dict {
    schema_conversion(crate::date::parse_date(&value), "schema.date")
}

#[func]
pub(super) fn tola_parse_partial_date(value: Value) -> Dict {
    use crate::date::PartialDate;

    schema_conversion(
        crate::date::parse_partial_date(&value).map(|date| {
            let mut fields = Dict::new();
            let (precision, year) = match date {
                PartialDate::Year(year) => ("year", year),
                PartialDate::Month { year, month } => {
                    fields.insert("month".into(), month.into_value());
                    ("month", year)
                }
                PartialDate::Day { year, month, day } => {
                    fields.insert("month".into(), month.into_value());
                    fields.insert("day".into(), day.into_value());
                    ("day", year)
                }
            };
            fields.insert("precision".into(), precision.into_value());
            fields.insert("year".into(), year.into_value());
            fields
        }),
        "schema.partial-date",
    )
}

#[func]
pub(super) fn tola_parse_timestamp(value: Value) -> Dict {
    schema_conversion(
        crate::date::Timestamp::parse(&value).map(|ts| {
            crate::dict([
                ("rfc3339", ts.rfc3339().into_value()),
                ("unix-seconds", ts.unix_seconds().into_value()),
                ("subsecond", ts.subsecond().into_value()),
            ])
        }),
        "schema.timestamp",
    )
}

// Conversion errors become an Issue until the schema supplies the field path and source.
fn schema_conversion<T: IntoValue, E: std::fmt::Display>(
    converted: Result<T, E>,
    code: &str,
) -> Dict {
    match converted {
        Ok(val) => crate::dict([("ok", true.into_value()), ("value", val.into_value())]),
        Err(err) => {
            let issue = crate::dict([
                ("code", code.into_value()),
                ("path", Array::new().into_value()),
                ("message", err.to_string().into_value()),
                ("notes", Array::new().into_value()),
                ("children", Array::new().into_value()),
            ]);
            crate::dict([
                ("ok", false.into_value()),
                (
                    "issues",
                    std::iter::once(issue.into_value())
                        .collect::<Array>()
                        .into_value(),
                ),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use typst::foundations::{Func, NativeFunc};

    use super::*;

    #[test]
    fn capability_reads_take_no_args() {
        assert_eq!(Func::from(tola_sources::data()).params().count(), 0);
        assert_eq!(Func::from(tola_current::data()).params().count(), 0);
        assert_eq!(Func::from(tola_source::data()).params().count(), 0);
    }
}
