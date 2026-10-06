//! The Typst library the engine compiles each site with, and the host bindings it exposes.
//!
//! The engine supplies every site-specific value through `sys.inputs`, and the packages read them
//! back from the compilation they run in. A deferred function therefore sees the build it is
//! evaluated in rather than the state captured when it was exported.

use std::sync::Arc;

use tola_address::{OutputPath, SiteUrlMount, browser_url, route_for_output};
use typst::diag::{At, SourceResult};
use typst::engine::Engine;
use typst::foundations::{Array, Dict, Func, IntoValue, Module, NativeFunc, Scope, Str, Value};
use typst::syntax::Span;
use typst::utils::LazyHash;

use crate::protocol::{
    ASSET_ORIGINS_MEMBER, ASSET_URLS_MEMBER, HOST_MODULE, SOURCE_ORIGINS_MEMBER,
    SOURCE_RECORDS_BY_FILE_MEMBER, SOURCE_RECORDS_MEMBER,
};

/// The site-specific values one compilation is bound to.
///
/// Whoever decides whether a compiled result may be reused has to compare every field: a value
/// no comparison covers would let a build reuse a compilation against a world that has since
/// changed.
pub struct HostInputs {
    /// The `@tola/site` values the engine resolved from the site configuration.
    pub site: Dict,
    /// Final configured asset URLs, keyed by the declared site-root URL.
    pub asset_urls: Dict,
    /// Site-root source paths for exact-file asset declarations, keyed by declared URL.
    pub asset_origins: Dict,
    /// The ordered source records.
    pub source_records: Array,
    /// Lexical source file views keyed by source path.
    pub source_records_by_file: Dict,
    /// Each source's authoritative file and declaration range, keyed by source path.
    pub source_origins: Dict,
}

/// One immutable Typst library, with the host bindings for a single build.
pub struct SiteLibrary {
    library: Arc<LazyHash<typst::Library>>,
}

impl SiteLibrary {
    pub fn new(inputs: HostInputs) -> Self {
        // The host interface is a module the packages import. Each name is the package-facing
        // one, and Typst checks the imported names and argument types, so a mismatch is an
        // ordinary compile error.
        let mut host = native_scope();
        host.define("site", inputs.site);

        // Typst gives a world one channel for compiled-in state: `sys.inputs`. A module value
        // there is importable by a package, and unlike the global scope it stays unreachable to
        // an author who does not name this key.
        let mut values = Dict::new();
        values.insert(
            HOST_MODULE.into(),
            Value::Module(Module::new(HOST_MODULE, host)),
        );
        values.insert(
            SOURCE_RECORDS_MEMBER.into(),
            inputs.source_records.into_value(),
        );
        values.insert(
            SOURCE_RECORDS_BY_FILE_MEMBER.into(),
            inputs.source_records_by_file.into_value(),
        );
        values.insert(
            SOURCE_ORIGINS_MEMBER.into(),
            inputs.source_origins.into_value(),
        );
        values.insert(ASSET_URLS_MEMBER.into(), inputs.asset_urls.into_value());
        values.insert(
            ASSET_ORIGINS_MEMBER.into(),
            inputs.asset_origins.into_value(),
        );

        Self {
            library: Arc::new(tola_typst::create_library_with_inputs(values)),
        }
    }

    pub fn shared(&self) -> Arc<LazyHash<typst::Library>> {
        Arc::clone(&self.library)
    }
}

/// The `all-sources` native in the host scope, which `@tola/source` re-exports: the one spelling
/// the engine and the language server both match against.
pub const ALL_SOURCES: &str = "all-sources";

/// The `current-source` native in the host scope, which `@tola/source` re-exports: the one
/// spelling the engine and the language server both match against.
pub const CURRENT_SOURCE: &str = "current-source";

/// The `tola-meta` native in the host scope, which `@tola/source` re-exports: the one spelling
/// the engine and the language server both match against.
pub const TOLA_META: &str = "tola-meta";

pub(crate) fn native_scope() -> Scope {
    let mut host = Scope::deduplicating();
    host.define("icon", Func::from(super::icons::tola_icon::data()));
    host.define(
        "icon-bytes",
        Func::from(super::icons::tola_icon_bytes::data()),
    );
    host.define("icon-url", Func::from(super::icons::tola_icon_url::data()));
    host.define(
        "resize-image",
        Func::from(super::images::resize_image::data()),
    );
    host.define(
        "image-metadata",
        Func::from(super::images::image_metadata::data()),
    );
    host.define(
        ALL_SOURCES,
        Func::from(super::natives::tola_sources::data()),
    );
    host.define(TOLA_META, Func::from(super::source_meta::tola_meta::data()));
    host.define(
        "report-source-issues",
        Func::from(super::source_issues::tola_report_source_issues::data()),
    );
    host.define(
        "current-document",
        Func::from(super::natives::tola_current::data()),
    );
    host.define(
        CURRENT_SOURCE,
        Func::from(super::natives::tola_source::data()),
    );
    host.define("slugify", Func::from(super::natives::tola_slug::data()));
    host.define("route", Func::from(super::natives::tola_route::data()));
    host.define(
        "route-to-output",
        Func::from(super::natives::tola_route_to_output::data()),
    );
    host.define(
        "output-to-route",
        Func::from(super::natives::tola_output_to_route::data()),
    );
    host.define(
        "decode-url-path",
        Func::from(super::natives::tola_decode_url_path::data()),
    );
    host.define(
        "plain-text",
        Func::from(super::text::tola_plain_text::data()),
    );
    host.define(
        "output-to-url",
        Func::from(super::natives::tola_output_to_url::data()),
    );
    host.define(
        "asset-url",
        Func::from(super::assets::tola_asset_url::data()),
    );
    host.define(
        "render-code",
        Func::from(super::code_render::tola_render_code::data()),
    );
    host.define(
        "code-stylesheet-url",
        Func::from(super::assets::tola_code_stylesheet_url::data()),
    );
    // Theme keys spell the theme name; values are package-anchored paths to the theme files.
    host.define("code-themes", super::code_themes::themes_dict());
    host.define("references", super::references::references_func());
    host
}

/// The browser URL one published output is served from, under the site's deployment mount.
pub(super) fn browser_output_url(
    engine: &Engine<'_>,
    span: Span,
    output: &OutputPath,
) -> SourceResult<String> {
    Ok(browser_url(
        &route_for_output(output),
        &site_mount(engine, span)?,
        None,
    ))
}

/// The deployment mount of the compilation `engine` is running.
pub(super) fn site_mount(engine: &Engine<'_>, span: Span) -> SourceResult<SiteUrlMount> {
    let base_path = site_base_path(engine)
        .ok_or("Tola site.base-path is unavailable in this compilation")
        .at(span)?;
    SiteUrlMount::from_base_path(base_path.as_str())
        .map_err(|error| error.to_string())
        .at(span)
}

/// The deployment base path for the compilation `engine` is running.
pub(super) fn site_base_path(engine: &Engine<'_>) -> Option<String> {
    site_value(engine, "base-path").map(|value| value.as_str().to_owned())
}

/// One string member of the `@tola/site` values this compilation is bound to.
fn site_value<'a>(engine: &Engine<'a>, member: &str) -> Option<&'a Str> {
    let Ok(Value::Module(host)) = sys_inputs(engine)?.get(HOST_MODULE) else {
        return None;
    };
    let Value::Dict(site) = host.scope().get("site")?.read() else {
        return None;
    };
    match site.get(member) {
        Ok(Value::Str(value)) => Some(value),
        _ => None,
    }
}

/// The `sys.inputs` of the compilation `engine` is running.
///
/// Typst exposes no accessor for them, and a deferred function must reach the inputs of the
/// compilation it runs in rather than a captured `sys` module.
fn sys_inputs<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    let Value::Module(system) = engine.library.global.scope().get("sys")?.read() else {
        return None;
    };
    let Value::Dict(inputs) = system.scope().get("inputs")?.read() else {
        return None;
    };
    Some(inputs)
}

/// The ordered source records of the compilation `engine` is running.
pub(super) fn source_records<'a>(engine: &Engine<'a>) -> Option<&'a Array> {
    match sys_inputs(engine)?.get(SOURCE_RECORDS_MEMBER) {
        Ok(Value::Array(sources)) => Some(sources),
        _ => None,
    }
}

/// Lexical source file views of this compilation, keyed by source path.
pub(super) fn source_records_by_file<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    match sys_inputs(engine)?.get(SOURCE_RECORDS_BY_FILE_MEMBER) {
        Ok(Value::Dict(by_file)) => Some(by_file),
        _ => None,
    }
}

/// Each source's authoritative file and declaration range, keyed by source path.
///
/// A value is `(file: <site-root path>, range: none | (start, end))`; `none` is a source that
/// declares no metadata. A caller takes both the diagnostic's file and its range from this one
/// entry, so the two always describe the same source snapshot.
pub(super) fn source_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    match sys_inputs(engine)?.get(SOURCE_ORIGINS_MEMBER) {
        Ok(Value::Dict(origins)) => Some(origins),
        _ => None,
    }
}

/// Final configured asset URLs for the compilation `engine` is running, keyed by the site-root
/// URL the site declares.
///
/// A value is the mounted browser URL, with the identity of the published bytes appended when
/// the site asked `[assets] cache-busting` and this compilation is a build. A read-only check
/// renders no bytes, so it resolves every declaration without one.
pub(super) fn asset_urls<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    match sys_inputs(engine)?.get(ASSET_URLS_MEMBER) {
        Ok(Value::Dict(urls)) => Some(urls),
        _ => None,
    }
}

pub(super) fn asset_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    match sys_inputs(engine)?.get(ASSET_ORIGINS_MEMBER) {
        Ok(Value::Dict(origins)) => Some(origins),
        _ => None,
    }
}
