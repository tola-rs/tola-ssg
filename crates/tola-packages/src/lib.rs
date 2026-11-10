//! The builtin `@tola/*` packages a site author imports.
//!
//! A package is an identity, the sources it carries, and the values the engine supplies to it. The
//! manifest every package needs is generated from that identity, so a package cannot carry a
//! manifest that disagrees with the specification it is resolved by.
//!
//! The engine, the language service, and the editor integration all read this crate: it is the
//! one owner of what a builtin package is, which of them a site imports, and how the engine
//! and the packages exchange values.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use typst::foundations::Str;

mod assets;
mod builtin;
mod code_render;
mod code_themes;
mod datalog;
pub mod date;
mod diagnostic;
mod docs;
mod hash;
mod icons;
mod image_request;
mod images;
mod issue;
pub mod library;
mod modules;
mod natives;
mod protocol;
mod references;
mod report;
pub mod schema_issue;
mod source_issues;
mod source_meta;
mod text;
mod uuid;

pub use assets::{
    ARGUMENT_DOMAINS, ArgumentDomain, ArgumentDomainDeclaration, CODE_STYLESHEET_FILE,
    code_stylesheet_output,
};
pub use builtin::{
    BuiltinPackage, PARSE_SOURCES, SOURCE_PACKAGE, TOLA_NAMESPACE, TolaPackage, builtin_package,
    builtin_packages, resolvable_packages,
};
pub use code_themes::{
    CodeTheme, THEME_DIRECTORY, ThemeAppearance, ThemeSource, theme_file, theme_names, themes,
    themes_dict,
};
pub use diagnostic::{DECLARATION_PROTOCOL_ORIGIN, SOURCE_ISSUE_ORIGIN};
pub use docs::{
    DocumentationSegment, ExportDeclaration, ExportDocumentation, ExportDocumentationError,
    Parameter, ParameterKind, RelatedTarget, Signature, cast_spelling, documentation_segments,
    type_spelling,
};
pub use icons::{
    PublishedIcon, collection_of_icon_file, icon_collection_names_bytes, icon_file_bytes,
    is_icon_file, published_icon_path, published_icon_request,
};
pub use image_request::{
    DescriptorError, ImageRequest, PackageSpecMessage, observation_bytes, owns_observation,
};
pub use images::AssetUrlShadowed;
pub use library::{ALL_SOURCES, CURRENT_SOURCE, TOLA_META};
pub use protocol::{
    ASSET_URLS_MEMBER, CAPABILITY_OBSERVATION_PATH, HOST_MODULE, ICON_OBSERVATION_DIRECTORY,
    ICON_REQUEST_OBSERVATION_DIRECTORY, IMAGE_REQUEST_OBSERVATION_DIRECTORY,
    IMAGE_REQUEST_OBSERVATION_EXTENSION, MODULES_MEMBER, SOURCE_OBSERVATION_DIRECTORY,
    SOURCE_RECORDS_BY_FILE_MEMBER, SOURCE_RECORDS_MEMBER,
};
pub use report::{REPORT_ORIGIN, ReportCaller, ReportOrigin, ReportSeverity};
pub use source_meta::{
    CaptureError, CaptureHeader, Declaration, DeclaredMeta, SourceCapture, decode_capture,
};

/// The key an author attribute is written under; `class` and `style` keep the element's spelling.
pub(crate) fn attribute_key(name: &Str) -> &str {
    if name.as_str().eq_ignore_ascii_case("class") {
        "class"
    } else if name.as_str().eq_ignore_ascii_case("style") {
        "style"
    } else {
        name.as_str()
    }
}

/// Join one author value onto what the element already sets: `class` is a list, `style` a block.
pub(crate) fn merge_attribute(attributes: &mut BTreeMap<String, String>, name: &str, value: &str) {
    let merged = match (name, attributes.get(name)) {
        ("class", Some(current)) => format!("{current} {value}"),
        ("style", Some(current)) => format!("{};{value}", current.trim_end_matches(';')),
        _ => value.to_owned(),
    };
    attributes.insert(name.to_owned(), merged);
}
