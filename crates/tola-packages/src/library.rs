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
    ASSET_ORIGINS_MEMBER, ASSET_URLS_MEMBER, HOST_MODULE, MODULES_MEMBER, SOURCE_ORIGINS_MEMBER,
    SOURCE_RECORDS_BY_FILE_MEMBER, SOURCE_RECORDS_MEMBER,
};

/// The site-specific values one compilation is bound to; every field takes part in reuse.
pub struct HostInputs {
    /// The `@tola/site` values the engine resolved from the site configuration.
    pub site: Dict,
    /// Configured module records keyed by the site's module aliases.
    pub modules: Dict,
    /// Final configured asset URLs, keyed by the declared site-root URL.
    pub asset_urls: Dict,
    /// Site-root source paths for exact-file asset declarations, keyed by declared URL.
    pub asset_origins: Dict,
    /// The ordered source records.
    pub source_records: Array,
    /// Lexical source file views keyed by source path.
    pub source_records_by_file: Dict,
    /// Each source's declaration file and range, keyed by source path.
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
        values.insert(MODULES_MEMBER.into(), inputs.modules.into_value());
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
    for (name, native) in [
        ("uuid-normalize", super::uuid::normalize::data()),
        ("uuid-v5", super::uuid::v5::data()),
        ("project-path", super::natives::project_path::data()),
        ("get-module", super::modules::get_module::data()),
        ("report-error", super::diagnostic::report_error::data()),
        ("report", super::report::report::data()),
        ("icon", super::icons::tola_icon::data()),
        ("icon-bytes", super::icons::tola_icon_bytes::data()),
        ("icon-url", super::icons::tola_icon_url::data()),
        ("resize-image", super::images::resize_image::data()),
        ("image-metadata", super::images::image_metadata::data()),
        ("image-srcset", super::images::image_srcset::data()),
        ("img", super::images::img::data()),
        (ALL_SOURCES, super::natives::tola_sources::data()),
        (TOLA_META, super::source_meta::tola_meta::data()),
        (
            "report-schema-error",
            super::source_issues::schema::tola_report_schema_error::data(),
        ),
        (
            "parse-source",
            super::source_issues::schema::tola_parse_source::data(),
        ),
        (
            "report-source-issues",
            super::source_issues::report::tola_report_source_issues::data(),
        ),
        (
            "normalize-schema-issues",
            super::schema_issue::normalize_schema_issues::data(),
        ),
        (
            "format-schema-issues",
            super::schema_issue::format_schema_issues::data(),
        ),
        ("current-document", super::natives::tola_current::data()),
        (CURRENT_SOURCE, super::natives::tola_source::data()),
        ("slugify", super::natives::tola_slug::data()),
        ("join-route", super::natives::tola_join_route::data()),
        ("output-of", super::natives::tola_output_of::data()),
        ("route-of", super::natives::tola_route_of::data()),
        (
            "decode-url-path",
            super::natives::tola_decode_url_path::data(),
        ),
        (
            "parse-url-path",
            super::natives::tola_parse_url_path::data(),
        ),
        ("parse-date", super::natives::tola_parse_date::data()),
        (
            "parse-partial-date",
            super::natives::tola_parse_partial_date::data(),
        ),
        (
            "parse-timestamp",
            super::natives::tola_parse_timestamp::data(),
        ),
        ("plain-text", super::text::tola_plain_text::data()),
        ("blake3", super::hash::tola_blake3::data()),
        ("bytes-to-hex", super::hash::tola_bytes_to_hex::data()),
        ("output-url", super::natives::tola_output_url::data()),
        ("asset-url", super::assets::tola_asset_url::data()),
        ("render-code", super::code_render::tola_render_code::data()),
        (
            "code-language",
            super::code_syntaxes::tola_code_language::data(),
        ),
        (
            "code-stylesheet-url",
            super::assets::tola_code_stylesheet_url::data(),
        ),
    ] {
        host.define(name, Func::from(native));
    }
    super::datalog::define(&mut host);
    // Theme keys spell the theme name; values are package-anchored paths to the theme files.
    host.define("code-themes", super::code_themes::themes_dict());
    host.define("code-syntaxes", super::code_syntaxes::files());
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

/// The `sys.inputs` of the running compilation; a deferred function must reach its own inputs.
fn sys_inputs<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    let Value::Module(system) = engine.library.global.scope().get("sys")?.read() else {
        return None;
    };
    let Value::Dict(inputs) = system.scope().get("inputs")?.read() else {
        return None;
    };
    Some(inputs)
}

/// Configured module records bound to this compilation.
pub(super) fn module_records<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, MODULES_MEMBER)
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
    dict_member(engine, SOURCE_RECORDS_BY_FILE_MEMBER)
}

/// Each source's declaration file and range, keyed by source path.
pub(super) fn source_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, SOURCE_ORIGINS_MEMBER)
}

/// Final asset URLs of the running compilation, keyed by declared URL; a check appends no identity.
pub(super) fn asset_urls<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, ASSET_URLS_MEMBER)
}

pub(super) fn asset_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, ASSET_ORIGINS_MEMBER)
}

/// One dictionary member of the `sys.inputs` this compilation is bound to.
fn dict_member<'a>(engine: &Engine<'a>, member: &str) -> Option<&'a Dict> {
    match sys_inputs(engine)?.get(member) {
        Ok(Value::Dict(value)) => Some(value),
        _ => None,
    }
}
