//! The native functions the builtin packages import from `@tola/host`.

use std::sync::LazyLock;

use typst::World;
use typst::comemo::Tracked;
use typst::diag::{At, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Array, Context, Dict, IntoValue, Str, Value, func};
use typst::introspection::{DocumentIntrospection, PathIntrospection, here};
use typst::syntax::{FileId, RootedPath, Span, Spanned, VirtualPath, VirtualRoot};

use crate::TolaPackage;
use crate::protocol::{CAPABILITY_OBSERVATION_PATH, SOURCE_OBSERVATION_DIRECTORY};

static SOURCES_CAPABILITY_FILE_ID: LazyLock<FileId> = LazyLock::new(|| {
    FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Source.spec()),
        VirtualPath::new(CAPABILITY_OBSERVATION_PATH)
            .expect("capability observation path is virtualizable"),
    ))
});

#[func]
pub(super) fn tola_sources(engine: &mut Engine, span: Span) -> SourceResult<Array> {
    // Typst keys evaluation by the current library. This read also records the
    // sources dependency for Tola's retained source-analysis entries.
    engine.world.source(*SOURCES_CAPABILITY_FILE_ID).at(span)?;
    let Some(sources) = super::library::source_records(engine) else {
        bail!(span, "Tola could not read the site's content sources");
    };
    Ok(sources.clone())
}

#[func(contextual)]
pub(super) fn tola_current(
    engine: &mut Engine,
    context: Tracked<Context>,
    span: Span,
) -> SourceResult<Dict> {
    let location = here(context).at(span)?;
    let document = engine.introspect(DocumentIntrospection(location, span));
    let path = engine.introspect(PathIntrospection(location, span));
    let (Some(document), Some(path)) = (document, path) else {
        bail!(
            span,
            "`current-document()` is called outside a `document(…)` body";
            hint: "call it in `context` inside a `document(…)` body"
        );
    };
    let path = path.get_with_slash();
    let output = tola_address::OutputPath::parse(path.trim_start_matches('/'))
        .map_err(|error| error.to_string())
        .at(span)?;
    let route = tola_address::route_for_output(&output);
    let values = [
        output.as_str().into_value(),
        route.as_str().into_value(),
        document.into_value(),
    ];
    let mut current = Dict::new();
    for (name, value) in CURRENT_DOCUMENT_FIELDS.into_iter().zip(values) {
        current.insert(name.into(), value);
    }
    Ok(current)
}

/// The fields `current-document()` publishes, in the order it inserts them.
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
        VirtualRoot::Package(package) => bail!(
            span,
            "`current-source()` is written in the package `{package}`";
            hint: "call it in the site's own content source and pass the value on"
        ),
    }
    let observation = FileId::new(RootedPath::new(
        VirtualRoot::Package(TolaPackage::Source.spec()),
        VirtualPath::new(format!(
            "{SOURCE_OBSERVATION_DIRECTORY}{}",
            id.vpath().get_with_slash()
        ))
        .expect("a source observation extends a validated virtual path"),
    ));
    engine.world.source(observation).at(span)?;
    let Some(by_file) = super::library::source_records_by_file(engine) else {
        bail!(span, "Tola could not read the site's content sources");
    };
    let Ok(record) = by_file.get(id.vpath().get_with_slash()).cloned() else {
        bail!(
            span,
            "`{}` is not a content source",
            id.vpath().get_with_slash();
            hint: "call `current-source()` in a source under `build.content-dir`"
        );
    };
    record.cast::<Dict>().at(span)
}

fn naming_rules(
    mode: Spanned<Str>,
    case: Spanned<Str>,
    separator: Spanned<Str>,
    language: &Str,
) -> SourceResult<tola_slugify::NamingRules> {
    use tola_slugify::{NamingRules, SlugCase, SlugMode, SlugSeparator};

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
        pronunciations: tola_slugify::HanPronunciations::for_language(language.as_str()),
    })
}

#[func]
pub(super) fn tola_slug(
    /// The text to turn into a slug.
    text: Spanned<Str>,
    /// The normalization mode: `unicode` or `ascii`.
    #[named]
    #[default(Spanned::detached("unicode".into()))]
    mode: Spanned<Str>,
    /// The case the name takes: `lower`, `upper`, `capitalize`, or `preserve`.
    #[named]
    #[default(Spanned::detached("lower".into()))]
    case: Spanned<Str>,
    /// The separator between words: `-` or `_`.
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
    use tola_slugify::slugify_segment;

    let rules = naming_rules(mode, case, separator, &language)?;
    let slug = slugify_segment(text.v.as_str(), rules)
        .ok_or("nothing in this text can name a segment")
        .at(text.span)?;
    Ok(slug.into())
}

#[func]
pub(super) fn tola_route(span: Span, segments: Array) -> SourceResult<Str> {
    let segments = route_segments_from_array(&segments).at(span)?;
    let mut route = String::from("/");
    for (position, segment) in segments.iter().enumerate() {
        if segment.contains(['/', '\\']) {
            bail!(
                span,
                "route segment {} (`{segment}`) names more than one directory",
                position + 1;
                hint: "pass one segment per element"
            );
        }
        if position > 0 {
            route.push('/');
        }
        route.push_str(segment);
    }
    if !segments.is_empty() {
        route.push('/');
    }
    let route = tola_address::UrlPath::from_decoded(&route)
        .map_err(|error| error.to_string())
        .at(span)?;
    Ok(route.as_str().into())
}

fn route_segments_from_array(segments: &Array) -> Result<Vec<String>, String> {
    segments
        .iter()
        .enumerate()
        .map(|(position, segment)| match segment {
            Value::Str(text) => Ok(text.as_str().to_owned()),
            other => Err(format!(
                "route segment {} must be a string, found {}",
                position + 1,
                other.ty().short_name()
            )),
        })
        .collect()
}

#[func]
pub(super) fn tola_route_to_output(route: Spanned<Str>) -> SourceResult<Str> {
    let route = tola_address::UrlPath::from_decoded(route.v.as_str())
        .map_err(|error| error.to_string())
        .at(route.span)?;
    Ok(tola_address::OutputPath::from_route(&route).as_str().into())
}

#[func]
pub(super) fn tola_output_to_route(output: Spanned<Str>) -> SourceResult<Str> {
    let output = tola_address::OutputPath::parse(output.v.as_str())
        .map_err(|error| error.to_string())
        .at(output.span)?;
    Ok(tola_address::route_for_output(&output).as_str().into())
}

#[func]
pub(super) fn tola_output_to_url(
    span: Span,
    output: Spanned<Str>,
    #[named]
    #[default("/".into())]
    base_path: Str,
    #[named]
    #[default]
    origin: Option<Str>,
) -> SourceResult<Str> {
    let output = tola_address::OutputPath::parse(output.v.as_str())
        .map_err(|error| error.to_string())
        .at(output.span)?;
    let mount = tola_address::SiteUrlMount::from_base_path(base_path.as_str())
        .map_err(|error| error.to_string())
        .at(span)?;
    let origin = origin
        .as_ref()
        .map(|origin| {
            let origin = tola_address::SiteOrigin::parse(origin.as_str())
                .map_err(|error| error.to_string())?;
            if origin.base_path().is_some() {
                return Err("`origin` must name only an http/https origin; pass the deployment path with `base-path`".to_owned());
            }
            Ok(origin)
        })
        .transpose()
        .at(span)?;
    let route = tola_address::route_for_output(&output);
    Ok(tola_address::browser_url(&route, &mount, origin.as_ref()).into())
}

#[func]
pub(super) fn tola_decode_url_path(url_path: Spanned<Str>) -> SourceResult<Str> {
    let path = tola_address::UrlPath::parse(url_path.v.as_str())
        .map_err(|error| error.to_string())
        .at(url_path.span)?;
    Ok(path.as_str().into())
}

#[cfg(test)]
mod tests {
    use typst::foundations::{Func, NativeFunc};

    use super::*;

    /// The host ABI the packages import: these three read the compilation they run in.
    #[test]
    fn capability_reads_take_no_arguments() {
        assert_eq!(Func::from(tola_sources::data()).params().count(), 0);
        assert_eq!(Func::from(tola_current::data()).params().count(), 0);
        assert_eq!(Func::from(tola_source::data()).params().count(), 0);
    }
}
