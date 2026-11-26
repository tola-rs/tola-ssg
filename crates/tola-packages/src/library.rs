//! Typst library and `sys.inputs` host bindings for one build.
//!
//! Site-specific values live in `sys.inputs` so deferred functions read the active compilation
//! rather than state captured when exported.

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

/// Site-specific inputs bound to one compilation.
pub struct HostInputs {
    /// `@tola/site` values resolved from the site configuration.
    pub site: Dict,
    /// Configured module records keyed by module alias.
    pub modules: Dict,
    /// Final configured asset URLs keyed by declared site-root URL.
    pub asset_urls: Dict,
    /// Site-root source paths for exact-file asset declarations, keyed by declared URL.
    pub asset_origins: Dict,
    /// Ordered source records.
    pub source_records: Array,
    /// Lexical source file views keyed by source path.
    pub source_records_by_file: Dict,
    /// Each source's declaration file and range, keyed by source path.
    pub source_origins: Dict,
}

/// Immutable Typst library with host bindings for a single build.
pub struct SiteLibrary {
    library: Arc<LazyHash<typst::Library>>,
}

impl SiteLibrary {
    pub fn new(inputs: HostInputs) -> Self {
        let mut host = native_scope();
        host.define("site", inputs.site);

        // `sys.inputs` keeps the internal host module unreachable unless explicitly imported.
        let values = crate::dict([
            (HOST_MODULE, Value::Module(Module::new(HOST_MODULE, host))),
            (SOURCE_RECORDS_MEMBER, inputs.source_records.into_value()),
            (
                SOURCE_RECORDS_BY_FILE_MEMBER,
                inputs.source_records_by_file.into_value(),
            ),
            (SOURCE_ORIGINS_MEMBER, inputs.source_origins.into_value()),
            (MODULES_MEMBER, inputs.modules.into_value()),
            (ASSET_URLS_MEMBER, inputs.asset_urls.into_value()),
            (ASSET_ORIGINS_MEMBER, inputs.asset_origins.into_value()),
        ]);

        let mut lib = tola_typst::create_library_with_inputs(values).into_inner();
        crate::code_syntaxes::install(&mut lib.rules);
        Self {
            library: Arc::new(lib.into()),
        }
    }

    pub fn shared(&self) -> Arc<LazyHash<typst::Library>> {
        Arc::clone(&self.library)
    }
}

/// Host scope name for `all-sources` re-exported by `@tola/source`.
pub const ALL_SOURCES: &str = "all-sources";

/// Host scope name for `current-source` re-exported by `@tola/source`.
pub const CURRENT_SOURCE: &str = "current-source";

/// Host scope name for `tola-meta` re-exported by `@tola/source`.
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
    host.define("code-themes", super::code_themes::themes_dict());
    host.define("code-syntaxes", super::code_syntaxes::files());
    host.define("references", super::references::references_func());
    host
}

pub(super) fn browser_output_url(
    engine: &Engine<'_>,
    span: Span,
    out: &OutputPath,
) -> SourceResult<String> {
    Ok(browser_url(
        &route_for_output(out),
        &site_mount(engine, span)?,
        None,
    ))
}

pub(super) fn site_mount(engine: &Engine<'_>, span: Span) -> SourceResult<SiteUrlMount> {
    let base = site_base_path(engine)
        .ok_or("Tola site.base-path is unavailable in this compilation")
        .at(span)?;
    SiteUrlMount::from_base_path(base.as_str())
        .map_err(|err| err.to_string())
        .at(span)
}

pub(super) fn site_base_path(engine: &Engine<'_>) -> Option<String> {
    site_value(engine, "base-path").map(|val| val.as_str().to_owned())
}

fn site_value<'a>(engine: &Engine<'a>, member: &str) -> Option<&'a Str> {
    let Ok(Value::Module(host)) = sys_inputs(engine)?.get(HOST_MODULE) else {
        return None;
    };
    let Value::Dict(site) = host.scope().get("site")?.read() else {
        return None;
    };
    match site.get(member) {
        Ok(Value::Str(val)) => Some(val),
        _ => None,
    }
}

fn sys_inputs<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    let Value::Module(sys) = engine.library.global.scope().get("sys")?.read() else {
        return None;
    };
    let Value::Dict(inputs) = sys.scope().get("inputs")?.read() else {
        return None;
    };
    Some(inputs)
}

pub(super) fn module_records<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, MODULES_MEMBER)
}

pub(super) fn source_records<'a>(engine: &Engine<'a>) -> Option<&'a Array> {
    crate::dict_array(sys_inputs(engine)?, SOURCE_RECORDS_MEMBER)
}

pub(super) fn source_records_by_file<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, SOURCE_RECORDS_BY_FILE_MEMBER)
}

pub(super) fn source_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, SOURCE_ORIGINS_MEMBER)
}

pub(super) fn asset_urls<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, ASSET_URLS_MEMBER)
}

pub(super) fn asset_origins<'a>(engine: &Engine<'a>) -> Option<&'a Dict> {
    dict_member(engine, ASSET_ORIGINS_MEMBER)
}

fn dict_member<'a>(engine: &Engine<'a>, member: &str) -> Option<&'a Dict> {
    crate::dict_dict(sys_inputs(engine)?, member)
}
