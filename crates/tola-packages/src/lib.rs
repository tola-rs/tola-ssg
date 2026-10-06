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

mod assets;
mod builtin;
mod code_render;
mod code_themes;
mod docs;
mod icons;
mod image_request;
mod images;
pub mod library;
mod natives;
mod protocol;
mod references;
mod source_issues;
mod source_meta;
mod text;

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
pub use docs::{
    DocumentationSegment, ExportDeclaration, ExportDocumentation, Parameter, ParameterKind,
    RelatedTarget, Signature, cast_spelling, documentation_segments, type_spelling,
};
pub use icons::{
    PublishedIcon, collection_of_icon_file, icon_file_bytes, is_icon_file, published_icon_path,
    published_icon_request,
};
pub use image_request::{ImageRequest, observation_bytes, owns_observation};
pub use images::AssetUrlShadowed;
pub use library::{ALL_SOURCES, CURRENT_SOURCE, TOLA_META};
pub use protocol::{
    ASSET_URLS_MEMBER, CAPABILITY_OBSERVATION_PATH, HOST_MODULE, ICON_OBSERVATION_DIRECTORY,
    ICON_REQUEST_OBSERVATION_DIRECTORY, IMAGE_REQUEST_OBSERVATION_DIRECTORY,
    IMAGE_REQUEST_OBSERVATION_EXTENSION, SOURCE_OBSERVATION_DIRECTORY,
    SOURCE_RECORDS_BY_FILE_MEMBER, SOURCE_RECORDS_MEMBER,
};
pub use source_meta::{
    CAPTURE_PROTOCOL, CaptureError, CaptureHeader, CaptureMode, DECLARATION_LABEL,
    DECLARATION_PROTOCOL, Declaration, DeclaredMeta, SourceCapture, decode_capture,
};
