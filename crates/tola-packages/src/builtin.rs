//! Builtin `@tola/*` packages, their embedded files, and generated manifests.

use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

use tola_typst::prelude::{PackageSpec, PackageVersion, ReadLocator};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, ast};

use crate::protocol::{CAPABILITY_OBSERVATION_PATH, SOURCE_OBSERVATION_DIRECTORY};

/// Namespace owning all builtin packages.
pub const TOLA_NAMESPACE: &str = "tola";

const TOLA_PACKAGE_VERSION: PackageVersion = PackageVersion {
    major: 0,
    minor: 0,
    patch: 0,
};

const MANIFEST_PATH: &str = "typst.toml";
const ENTRYPOINT_PATH: &str = "lib.typ";

type StaticFile = (&'static str, &'static str);

enum PackageSources {
    Fixed(&'static [StaticFile]),
    FixedThenThemes(&'static [StaticFile]),
}

impl PackageSources {
    fn len(&self) -> usize {
        match self {
            Self::Fixed(files) => files.len(),
            Self::FixedThenThemes(fixed) => fixed.len() + crate::code_themes::themes().len(),
        }
    }

    fn at(&self, idx: usize) -> Option<StaticFile> {
        match self {
            Self::Fixed(files) => files.get(idx).copied(),
            Self::FixedThenThemes(fixed) => fixed.get(idx).copied().or_else(|| {
                let theme = crate::code_themes::themes().get(idx - fixed.len())?;
                Some((theme.file, crate::code_themes::file_contents(theme.file)))
            }),
        }
    }

    fn iter(&self) -> impl Iterator<Item = StaticFile> + '_ {
        (0..self.len()).filter_map(|i| self.at(i))
    }
}

struct PackageDef {
    name: &'static str,
    sources: PackageSources,
}

const fn pkg_def(name: &'static str, files: &'static [StaticFile]) -> PackageDef {
    PackageDef {
        name,
        sources: PackageSources::Fixed(files),
    }
}

impl PackageDef {
    fn manifest(&self) -> String {
        format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\nentrypoint = \"{ENTRYPOINT_PATH}\"\n",
            self.name, TOLA_PACKAGE_VERSION
        )
    }

    fn files(&self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> + '_ {
        std::iter::once((MANIFEST_PATH, Cow::Owned(self.manifest())))
            .chain(self.sources.iter().map(|(p, s)| (p, Cow::Borrowed(s))))
    }

    fn file(&self, path: &Path) -> Option<Cow<'static, str>> {
        if path == Path::new(MANIFEST_PATH) {
            return Some(Cow::Owned(self.manifest()));
        }
        self.sources
            .iter()
            .find(|(p, _)| path == Path::new(p))
            .map(|(_, s)| Cow::Borrowed(s))
    }

    fn entrypoint(&self) -> &'static str {
        self.sources
            .iter()
            .find(|(p, _)| *p == ENTRYPOINT_PATH)
            .expect("every package carries its entrypoint")
            .1
    }
}

static PACKAGE_EXPORTS: LazyLock<Vec<Vec<&'static str>>> = LazyLock::new(|| {
    PACKAGE_DEFINITIONS
        .iter()
        .map(|def| export_names(def.entrypoint()))
        .collect()
});

fn export_names(module: &'static str) -> Vec<&'static str> {
    let src = Source::detached(module);
    let root = LinkedNode::new(src.root());
    let mut names = Vec::new();
    for stmt in root.children() {
        if let Some(binding) = stmt.cast::<ast::LetBinding>() {
            for name in binding.kind().bindings() {
                publish(&mut names, module, &stmt, name);
            }
        } else if let Some(import) = stmt.cast::<ast::ModuleImport>()
            && let Some(ast::Imports::Items(items)) = import.imports()
        {
            for item in items.iter() {
                publish(&mut names, module, &stmt, item.bound_name());
            }
        }
    }
    names
}

fn publish(
    names: &mut Vec<&'static str>,
    module: &'static str,
    stmt: &LinkedNode<'_>,
    name: ast::Ident<'_>,
) {
    if let Some(node) = stmt.find(name.span())
        && let Some(spelling) = module.get(node.range())
        && !names.contains(&spelling)
    {
        names.push(spelling);
    }
}

/// One row per `TolaPackage` variant in `TolaPackage::all()` order.
const PACKAGE_DEFINITIONS: [PackageDef; TolaPackage::all().len()] = [
    pkg_def("host", &[("lib.typ", include_str!("embed/host.typ"))]),
    pkg_def("site", &[("lib.typ", include_str!("embed/site.typ"))]),
    pkg_def("module", &[("lib.typ", include_str!("embed/module.typ"))]),
    pkg_def("address", &[("lib.typ", include_str!("embed/address.typ"))]),
    pkg_def("icon", &[("lib.typ", include_str!("embed/icon.typ"))]),
    pkg_def("image", &[("lib.typ", include_str!("embed/image.typ"))]),
    pkg_def("source", &[("lib.typ", include_str!("embed/source.typ"))]),
    pkg_def(
        "collection",
        &[
            ("lib.typ", include_str!("embed/collection.typ")),
            ("keys.typ", include_str!("embed/keys.typ")),
        ],
    ),
    pkg_def("datalog", &[("lib.typ", include_str!("embed/datalog.typ"))]),
    pkg_def(
        "schema",
        &[
            ("lib.typ", include_str!("embed/schema.typ")),
            ("requirements.typ", include_str!("embed/requirements.typ")),
            ("schema-rules.typ", include_str!("embed/schema-rules.typ")),
        ],
    ),
    pkg_def(
        "document",
        &[("lib.typ", include_str!("embed/document.typ"))],
    ),
    PackageDef {
        name: "code",
        sources: PackageSources::FixedThenThemes(&[
            ("lib.typ", include_str!("embed/code.typ")),
            (
                crate::CODE_STYLESHEET_FILE,
                include_str!("embed/code-stylesheet.css"),
            ),
        ]),
    },
    pkg_def(
        "head",
        &[
            ("lib.typ", include_str!("embed/head.typ")),
            ("head-fields.typ", include_str!("embed/head-fields.typ")),
        ],
    ),
    pkg_def("feed", &[("lib.typ", include_str!("embed/feed.typ"))]),
    pkg_def("sitemap", &[("lib.typ", include_str!("embed/sitemap.typ"))]),
    pkg_def("hash", &[("lib.typ", include_str!("embed/hash.typ"))]),
    pkg_def("uuid", &[("lib.typ", include_str!("embed/uuid.typ"))]),
    pkg_def(
        "diagnostic",
        &[("lib.typ", include_str!("embed/diagnostic.typ"))],
    ),
];

const _: () = {
    assert!(PACKAGE_DEFINITIONS.len() == TolaPackage::all().len());
    let mut i = 0;
    while i < PACKAGE_DEFINITIONS.len() {
        assert!(TolaPackage::all()[i] as usize == i);
        i += 1;
    }
};

/// Builtin `@tola/*` package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TolaPackage {
    Host,
    Site,
    Module,
    Address,
    Icon,
    Image,
    Source,
    Collection,
    Datalog,
    Schema,
    Document,
    Code,
    Head,
    Feed,
    Sitemap,
    Hash,
    Uuid,
    Diagnostic,
}

/// Package name for `@tola/source`.
pub const SOURCE_PACKAGE: &str = "source";

/// Export name for `parse-sources` in `@tola/source`.
pub const PARSE_SOURCES: &str = "parse-sources";

impl TolaPackage {
    pub const fn name(self) -> &'static str {
        self.definition().name
    }

    const fn definition(self) -> &'static PackageDef {
        &PACKAGE_DEFINITIONS[self as usize]
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::Host,
            Self::Site,
            Self::Module,
            Self::Address,
            Self::Icon,
            Self::Image,
            Self::Source,
            Self::Collection,
            Self::Datalog,
            Self::Schema,
            Self::Document,
            Self::Code,
            Self::Head,
            Self::Feed,
            Self::Sitemap,
            Self::Hash,
            Self::Uuid,
            Self::Diagnostic,
        ]
    }

    pub fn from_spec(spec: &PackageSpec) -> Option<Self> {
        if spec.namespace.as_str() != TOLA_NAMESPACE || spec.version != TOLA_PACKAGE_VERSION {
            return None;
        }
        Self::all()
            .iter()
            .copied()
            .find(|pkg| pkg.name() == spec.name.as_str())
    }

    pub fn spec(self) -> PackageSpec {
        PackageSpec {
            namespace: TOLA_NAMESPACE.into(),
            name: self.name().into(),
            version: TOLA_PACKAGE_VERSION,
        }
    }

    pub(crate) const fn is_engine_only(self) -> bool {
        match self {
            Self::Host => true,
            Self::Site
            | Self::Module
            | Self::Address
            | Self::Icon
            | Self::Image
            | Self::Source
            | Self::Collection
            | Self::Datalog
            | Self::Schema
            | Self::Document
            | Self::Code
            | Self::Head
            | Self::Feed
            | Self::Sitemap
            | Self::Hash
            | Self::Uuid
            | Self::Diagnostic => false,
        }
    }

    pub fn files(self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> {
        self.definition().files()
    }

    pub fn file(self, path: &Path) -> Option<Cow<'static, str>> {
        self.definition().file(path)
    }

    pub fn exports(self) -> impl Iterator<Item = &'static str> {
        PACKAGE_EXPORTS[self as usize].iter().copied()
    }

    pub fn owns_capability_read(self, locator: &ReadLocator) -> bool {
        matches!(
            locator,
            ReadLocator::Package { package, path }
                | ReadLocator::ProvidedPackage { package, path }
                if TolaPackage::from_spec(package) == Some(self)
                    && path == Path::new(CAPABILITY_OBSERVATION_PATH)
        )
    }

    pub fn observed_source_path(self, path: &Path) -> Option<&Path> {
        if self != Self::Source {
            return None;
        }
        path.strip_prefix(SOURCE_OBSERVATION_DIRECTORY)
            .ok()
            .filter(|file| !file.as_os_str().is_empty())
    }

    pub fn is_observation_descriptor(self, path: &Path) -> bool {
        self == Self::Source
            && (path == Path::new(CAPABILITY_OBSERVATION_PATH)
                || self.observed_source_path(path).is_some())
    }
}

/// Builtin package as imported by a site author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinPackage {
    pkg: TolaPackage,
}

impl BuiltinPackage {
    pub fn spec(&self) -> PackageSpec {
        self.pkg.spec()
    }

    pub fn host_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn files(&self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> {
        self.pkg.files()
    }

    pub fn exports(&self) -> impl Iterator<Item = &'static str> {
        self.pkg.exports()
    }
}

/// All builtin packages resolved by the compiler, including engine-only packages.
pub fn resolvable_packages() -> impl Iterator<Item = BuiltinPackage> {
    TolaPackage::all()
        .iter()
        .copied()
        .map(|pkg| BuiltinPackage { pkg })
}

/// Builtin packages intended for direct import by site authors.
pub fn builtin_packages() -> impl Iterator<Item = BuiltinPackage> {
    resolvable_packages().filter(|b| !b.pkg.is_engine_only())
}

pub fn builtin_package(spec: &PackageSpec) -> Option<BuiltinPackage> {
    TolaPackage::from_spec(spec).map(|pkg| BuiltinPackage { pkg })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_and_fields_are_hyphenated() {
        for pkg in TolaPackage::all().iter().copied() {
            let builtin = builtin_package(&pkg.spec()).expect("every builtin package resolves");
            for name in builtin.exports() {
                assert!(
                    hyphenated(name),
                    "@tola/{} publishes `{name}` without hyphenated spelling",
                    pkg.name()
                );
                let docs = builtin
                    .export_documentation(&[name.to_owned()])
                    .unwrap_or_else(|err| panic!("@tola/{} documents `{name}`: {err}", pkg.name()));
                for export in docs {
                    if let crate::docs::ExportDeclaration::Function(sig) = export.declaration {
                        for param in sig.parameters {
                            assert!(
                                hyphenated(&param.name),
                                "@tola/{} `{name}` parameter `{}` is not hyphenated",
                                pkg.name(),
                                param.name
                            );
                        }
                    }
                }
            }
        }
        for (api, fields) in [
            ("image-metadata", &crate::images::IMAGE_METADATA_FIELDS[..]),
            ("resize-image", &crate::images::RESIZE_IMAGE_FIELDS[..]),
            ("image-srcset", &crate::images::IMAGE_SRCSET_FIELDS[..]),
            (
                "current-document",
                &crate::natives::CURRENT_DOCUMENT_FIELDS[..],
            ),
        ] {
            for field in fields {
                assert!(
                    hyphenated(field),
                    "`{api}` publishes `{field}` without hyphenated spelling"
                );
            }
        }
    }

    fn hyphenated(name: &str) -> bool {
        let mut words = name.split('-');
        let Some(first) = words.next() else {
            return false;
        };
        let word = |w: &str| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        };
        word(first) && words.all(word)
    }
}
