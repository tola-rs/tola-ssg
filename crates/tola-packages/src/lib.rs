//! Builtin `@tola/*` Typst packages, their embedded sources, and engine host bindings.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use typst::foundations::{Array, Dict, Str, Value};
use typst::syntax::{FileId, VirtualRoot};

mod assets;
mod builtin;
mod code_render;
mod code_syntaxes;
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
    ExportDeclaration, ExportDocumentation, ExportDocumentationError, Parameter, ParameterKind,
    RelatedTarget, Signature, cast_spelling, type_spelling,
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

pub(crate) fn dict<'a>(fields: impl IntoIterator<Item = (&'a str, Value)>) -> Dict {
    let mut out = Dict::new();
    for (k, v) in fields {
        out.insert(k.into(), v);
    }
    out
}

pub(crate) fn dict_str<'a>(dict: &'a Dict, key: &str) -> Option<&'a str> {
    match dict.get(key) {
        Ok(Value::Str(val)) => Some(val.as_str()),
        _ => None,
    }
}

pub(crate) fn dict_array<'a>(dict: &'a Dict, key: &str) -> Option<&'a Array> {
    match dict.get(key) {
        Ok(Value::Array(val)) => Some(val),
        _ => None,
    }
}

pub(crate) fn dict_dict<'a>(dict: &'a Dict, key: &str) -> Option<&'a Dict> {
    match dict.get(key) {
        Ok(Value::Dict(val)) => Some(val),
        _ => None,
    }
}

pub(crate) fn pkg_of_file(id: FileId) -> Option<TolaPackage> {
    match id.root() {
        VirtualRoot::Package(spec) => TolaPackage::from_spec(spec),
        VirtualRoot::Project => None,
    }
}

/// Canonicalizes `class` and `style` attribute keys to lowercase; preserves other names.
pub(crate) fn attr_key(name: &Str) -> &str {
    if name.as_str().eq_ignore_ascii_case("class") {
        "class"
    } else if name.as_str().eq_ignore_ascii_case("style") {
        "style"
    } else {
        name.as_str()
    }
}

/// Merges `value` into `attrs`, joining `class` with spaces and `style` with semicolons.
pub(crate) fn merge_attr(attrs: &mut BTreeMap<String, String>, name: &str, val: &str) {
    let merged = match (name, attrs.get(name)) {
        ("class", Some(cur)) => format!("{cur} {val}"),
        ("style", Some(cur)) => format!("{};{val}", cur.trim_end_matches(';')),
        _ => val.to_owned(),
    };
    attrs.insert(name.to_owned(), merged);
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;
    use std::sync::Arc;

    use tola_typst::{
        BundleCancellation, FileMap, FileProvider, FileStore, FileTarget, TypstWorld, file_id,
    };
    use typst::Library;
    use typst::foundations::{Array, Dict};
    use typst::syntax::{FileId, VirtualRoot};
    use typst::utils::LazyHash;

    use crate::library::HostInputs;
    use crate::{TOLA_NAMESPACE, TolaPackage};

    pub(crate) struct TestFiles(pub FileMap);

    impl FileProvider for TestFiles {
        fn target(&self, id: FileId) -> Option<FileTarget> {
            if let Some(target) = self.0.target(id) {
                return Some(target);
            }
            let VirtualRoot::Package(spec) = id.root() else {
                return None;
            };
            let path = Path::new(id.vpath().get_without_slash());
            TolaPackage::from_spec(spec)
                .and_then(|pkg| pkg.file(path))
                .map(|src| FileTarget::Bytes(Arc::from(src.into_owned().into_bytes())))
        }

        fn owned_namespaces(&self) -> &'static [&'static str] {
            &[TOLA_NAMESPACE, "tola-modules"]
        }
    }

    pub(crate) fn empty_inputs() -> HostInputs {
        HostInputs {
            site: Dict::new(),
            modules: Dict::new(),
            asset_urls: Dict::new(),
            asset_origins: Dict::new(),
            source_records: Array::new(),
            source_records_by_file: Dict::new(),
            source_origins: Dict::new(),
        }
    }

    pub(crate) fn file_map(files: &[(&str, &str)]) -> FileMap {
        let mut map = FileMap::new();
        for (path, text) in files {
            map.insert(file_id(path), text.as_bytes().to_vec());
        }
        map
    }

    pub(crate) fn test_world(
        dir: &str,
        files: FileMap,
        library: Arc<LazyHash<Library>>,
    ) -> TypstWorld {
        let root = std::env::temp_dir().join(dir);
        TypstWorld::builder(&root.join("main.typ"), &root)
            .with_files(Arc::new(FileStore::new().with_provider(TestFiles(files))))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(library)
            .build(&BundleCancellation::default())
            .expect("valid test world")
    }
}
