//! The builtin `@tola/*` packages: their identity, their files, and the set a site imports.

use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

use tola_typst::prelude::{PackageSpec, PackageVersion, ReadLocator};
use typst::syntax::ast::AstNode;
use typst::syntax::{LinkedNode, Source, ast};

use crate::protocol::{CAPABILITY_OBSERVATION_PATH, SOURCE_OBSERVATION_DIRECTORY};

/// The namespace every builtin package belongs to.
///
/// Tola owns this namespace: only the packages this crate lists exist in it, and a package
/// directory holding one is ignored rather than used.
pub const TOLA_NAMESPACE: &str = "tola";
/// The fixed import version every builtin package uses.
///
/// The implementation version is this crate's own version, which
/// [`BuiltinPackage::host_version`] reports.
const TOLA_PACKAGE_VERSION: PackageVersion = PackageVersion {
    major: 0,
    minor: 0,
    patch: 0,
};

/// Where a builtin package's generated manifest lives.
const MANIFEST_PATH: &str = "typst.toml";

/// Where every builtin package's entrypoint lives.
const ENTRYPOINT_PATH: &str = "lib.typ";

/// One source file a builtin package carries.
#[derive(Clone, Copy)]
struct PackageSource {
    path: &'static str,
    contents: &'static str,
}

impl PackageSource {
    const fn new(path: &'static str, contents: &'static str) -> Self {
        Self { path, contents }
    }
}

/// The sources one builtin package carries.
enum PackageSources {
    /// A fixed list, written out with the package definition.
    Fixed(&'static [PackageSource]),
    /// The package's own files, then one file per theme of the code theme catalog.
    FixedThenThemes(&'static [PackageSource]),
}

impl PackageSources {
    fn len(&self) -> usize {
        match self {
            Self::Fixed(sources) => sources.len(),
            Self::FixedThenThemes(fixed) => fixed.len() + crate::code_themes::themes().len(),
        }
    }

    /// The file carried at `index`.
    fn at(&self, index: usize) -> Option<PackageSource> {
        match self {
            Self::Fixed(sources) => sources.get(index).copied(),
            Self::FixedThenThemes(fixed) => fixed.get(index).copied().or_else(|| {
                let theme = crate::code_themes::themes().get(index - fixed.len())?;
                Some(PackageSource::new(
                    theme.file,
                    crate::code_themes::file_contents(theme.file),
                ))
            }),
        }
    }

    fn iter(&self) -> impl Iterator<Item = PackageSource> + '_ {
        (0..self.len()).filter_map(|index| self.at(index))
    }
}

/// One builtin package: its name, and the sources it carries beside its generated manifest.
struct PackageDefinition {
    name: &'static str,
    sources: PackageSources,
}

impl PackageDefinition {
    /// The manifest Typst reads: the name and fixed import version are the only inputs, so a
    /// package cannot carry a manifest that disagrees with the identity it is resolved by.
    fn manifest(&self) -> String {
        format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\nentrypoint = \"{ENTRYPOINT_PATH}\"\n",
            self.name, TOLA_PACKAGE_VERSION
        )
    }

    /// Every file this package carries, manifest first.
    fn files(&self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> + '_ {
        std::iter::once((MANIFEST_PATH, Cow::Owned(self.manifest()))).chain(
            self.sources
                .iter()
                .map(|file| (file.path, Cow::Borrowed(file.contents))),
        )
    }

    /// The builtin file at the package-relative `path`, without a leading `/`.
    fn file(&self, path: &Path) -> Option<Cow<'static, str>> {
        if path == Path::new(MANIFEST_PATH) {
            return Some(Cow::Owned(self.manifest()));
        }
        self.sources
            .iter()
            .find(|file| path == Path::new(file.path))
            .map(|file| Cow::Borrowed(file.contents))
    }

    fn entrypoint(&self) -> &'static str {
        self.sources
            .iter()
            .find(|file| file.path == ENTRYPOINT_PATH)
            .expect("every package carries its entrypoint")
            .contents
    }
}

/// The names each entrypoint publishes, discovered once from its own syntax tree.
static PACKAGE_EXPORTS: LazyLock<Vec<Vec<&'static str>>> = LazyLock::new(|| {
    PACKAGE_DEFINITIONS
        .iter()
        .map(|definition| export_names(definition.entrypoint()))
        .collect()
});

/// The names one module source publishes: its top-level `#let` bindings and the members it
/// imports explicitly, in source order, each name once even when a later statement rebinds it.
///
/// A wildcard import publishes whatever the module it reads happens to hold, and a statement
/// inside a body publishes nothing, so neither can leak a helper or a standard-library name.
fn export_names(module: &'static str) -> Vec<&'static str> {
    let source = Source::detached(module);
    let root = LinkedNode::new(source.root());
    let mut names = Vec::new();
    for statement in root.children() {
        if let Some(binding) = statement.cast::<ast::LetBinding>() {
            for name in binding.kind().bindings() {
                publish(&mut names, module, &statement, name);
            }
        } else if let Some(import) = statement.cast::<ast::ModuleImport>()
            && let Some(ast::Imports::Items(items)) = import.imports()
        {
            for item in items.iter() {
                publish(&mut names, module, &statement, item.bound_name());
            }
        }
    }
    names
}

/// Add one bound name once, keeping the position of its first binding.
fn publish(
    names: &mut Vec<&'static str>,
    module: &'static str,
    statement: &LinkedNode<'_>,
    name: ast::Ident<'_>,
) {
    if let Some(spelling) = spelling(module, statement, name)
        && !names.contains(&spelling)
    {
        names.push(spelling);
    }
}

/// How one statement spells an identifier it binds.
fn spelling(
    module: &'static str,
    statement: &LinkedNode<'_>,
    name: ast::Ident<'_>,
) -> Option<&'static str> {
    module.get(statement.find(name.span())?.range())
}

/// Holds exactly one row per variant, in the order [`TolaPackage::all`] lists them.
const PACKAGE_DEFINITIONS: [PackageDefinition; 11] = [
    PackageDefinition {
        name: "host",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/host.typ"),
        )]),
    },
    PackageDefinition {
        name: "site",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/site.typ"),
        )]),
    },
    PackageDefinition {
        name: "address",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/address.typ"),
        )]),
    },
    PackageDefinition {
        name: "icon",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/icon.typ"),
        )]),
    },
    PackageDefinition {
        name: "image",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/image.typ"),
        )]),
    },
    PackageDefinition {
        name: "source",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/source.typ"),
        )]),
    },
    PackageDefinition {
        name: "collection",
        sources: PackageSources::Fixed(&[
            PackageSource::new("lib.typ", include_str!("embed/collection.typ")),
            PackageSource::new("keys.typ", include_str!("embed/keys.typ")),
        ]),
    },
    PackageDefinition {
        name: "schema",
        sources: PackageSources::Fixed(&[
            PackageSource::new("lib.typ", include_str!("embed/schema.typ")),
            PackageSource::new("requirements.typ", include_str!("embed/requirements.typ")),
            PackageSource::new("schema-rules.typ", include_str!("embed/schema-rules.typ")),
        ]),
    },
    PackageDefinition {
        name: "document",
        sources: PackageSources::Fixed(&[PackageSource::new(
            "lib.typ",
            include_str!("embed/document.typ"),
        )]),
    },
    PackageDefinition {
        name: "code",
        sources: PackageSources::FixedThenThemes(&[
            PackageSource::new("lib.typ", include_str!("embed/code.typ")),
            PackageSource::new(
                crate::CODE_STYLESHEET_FILE,
                include_str!("embed/code-stylesheet.css"),
            ),
        ]),
    },
    PackageDefinition {
        name: "web",
        sources: PackageSources::Fixed(&[
            PackageSource::new("lib.typ", include_str!("embed/web.typ")),
            PackageSource::new("web-fields.typ", include_str!("embed/web-fields.typ")),
        ]),
    },
];

const _: () = {
    assert!(PACKAGE_DEFINITIONS.len() == TolaPackage::all().len());
    let mut index = 0;
    while index < PACKAGE_DEFINITIONS.len() {
        assert!(TolaPackage::all()[index] as usize == index);
        index += 1;
    }
};

/// One builtin `@tola/*` package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TolaPackage {
    Host,
    Site,
    Address,
    Icon,
    Image,
    Source,
    Collection,
    Schema,
    Document,
    Code,
    Web,
}

/// The name of the `@tola/source` package: the one spelling the engine and the language server
/// both match against.
pub const SOURCE_PACKAGE: &str = "source";

/// The `parse-sources` export of `@tola/source`, the package's Typst-level metadata resolver: the
/// one spelling the engine and the language server both match against.
pub const PARSE_SOURCES: &str = "parse-sources";

impl TolaPackage {
    pub const fn name(self) -> &'static str {
        self.definition().name
    }

    const fn definition(self) -> &'static PackageDefinition {
        &PACKAGE_DEFINITIONS[self as usize]
    }

    /// Every builtin package, in declaration order.
    pub const fn all() -> &'static [Self] {
        &[
            Self::Host,
            Self::Site,
            Self::Address,
            Self::Icon,
            Self::Image,
            Self::Source,
            Self::Collection,
            Self::Schema,
            Self::Document,
            Self::Code,
            Self::Web,
        ]
    }

    /// Match from a Typst package specification.
    pub fn from_spec(spec: &PackageSpec) -> Option<Self> {
        if spec.namespace.as_str() != TOLA_NAMESPACE || spec.version != TOLA_PACKAGE_VERSION {
            return None;
        }

        Self::all()
            .iter()
            .copied()
            .find(|package| package.name() == spec.name.as_str())
    }

    /// The specification every read of this package's virtual files is rooted at.
    pub fn spec(self) -> PackageSpec {
        PackageSpec {
            namespace: TOLA_NAMESPACE.into(),
            name: self.name().into(),
            version: TOLA_PACKAGE_VERSION,
        }
    }

    /// Whether this package exists for Tola's own packages rather than for a site author.
    ///
    /// The match is exhaustive so a new package cannot become public by accident.
    pub(crate) const fn is_engine_only(self) -> bool {
        match self {
            Self::Host => true,
            Self::Site
            | Self::Address
            | Self::Icon
            | Self::Image
            | Self::Source
            | Self::Collection
            | Self::Schema
            | Self::Document
            | Self::Code
            | Self::Web => false,
        }
    }

    /// Every file this package carries, manifest first.
    pub fn files(self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> {
        self.definition().files()
    }

    /// The builtin file at the package-relative `path`, without a leading `/`.
    pub fn file(self, path: &Path) -> Option<Cow<'static, str>> {
        self.definition().file(path)
    }

    /// The names this package's entrypoint publishes.
    ///
    /// Read from the entrypoint's own syntax tree rather than a list, so an export cannot go
    /// missing from the code that defines it.
    pub fn exports(self) -> impl Iterator<Item = &'static str> {
        PACKAGE_EXPORTS[self as usize].iter().copied()
    }

    /// Whether `locator` reads this package's capability observation.
    pub fn owns_capability_read(self, locator: &ReadLocator) -> bool {
        matches!(
            locator,
            ReadLocator::Package { package, path }
                | ReadLocator::ProvidedPackage { package, path }
                if TolaPackage::from_spec(package) == Some(self)
                    && path == Path::new(CAPABILITY_OBSERVATION_PATH)
        )
    }

    /// The lexical source path encoded by a `source` package observation read.
    pub fn observed_source_path(self, path: &Path) -> Option<&Path> {
        if self != Self::Source {
            return None;
        }
        path.strip_prefix(SOURCE_OBSERVATION_DIRECTORY)
            .ok()
            .filter(|file| !file.as_os_str().is_empty())
    }

    /// Whether `path` names an immutable observation descriptor rather than builtin bytes.
    ///
    /// `source` owns every source observation: the package-wide capability read and the
    /// per-file lexical identity reads.
    pub fn is_observation_descriptor(self, path: &Path) -> bool {
        self == Self::Source
            && (path == Path::new(CAPABILITY_OBSERVATION_PATH)
                || self.observed_source_path(path).is_some())
    }
}

/// One builtin package as a site author imports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinPackage {
    package: TolaPackage,
}

impl BuiltinPackage {
    /// The specification an author imports.
    pub fn spec(&self) -> PackageSpec {
        self.package.spec()
    }

    /// Version of the crate that supplies this package's implementation.
    ///
    /// The fixed `0.0.0` import version is not the implementation version. Pair
    /// this identity with the embedded Typst version when describing the API.
    pub fn host_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Package-relative paths and the exact source consumed by the compiler.
    pub fn files(&self) -> impl Iterator<Item = (&'static str, Cow<'static, str>)> {
        self.package.files()
    }

    /// The names this package's entrypoint publishes.
    pub fn exports(&self) -> impl Iterator<Item = &'static str> {
        self.package.exports()
    }
}

/// Every builtin package the compiler resolves, including the ones that exist only for Tola's
/// own packages.
pub fn resolvable_packages() -> impl Iterator<Item = BuiltinPackage> {
    TolaPackage::all()
        .iter()
        .copied()
        .map(|package| BuiltinPackage { package })
}

/// The packages a site author imports.
///
/// Omits the ones that exist for Tola's own packages, which the compiler still resolves.
pub fn builtin_packages() -> impl Iterator<Item = BuiltinPackage> {
    resolvable_packages().filter(|package| !package.package.is_engine_only())
}

/// The builtin package a specification names, whether or not a site author imports it.
pub fn builtin_package(spec: &PackageSpec) -> Option<BuiltinPackage> {
    TolaPackage::from_spec(spec).map(|package| BuiltinPackage { package })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ALL_SOURCES, CURRENT_SOURCE, TOLA_META};

    #[test]
    fn each_package_carries_one_manifest() {
        for package in TolaPackage::all() {
            let paths = package.files().map(|(path, _)| path).collect::<Vec<_>>();
            assert_eq!(paths.first(), Some(&MANIFEST_PATH), "{}", package.name());
            assert_eq!(
                paths.iter().filter(|path| **path == MANIFEST_PATH).count(),
                1,
                "{}",
                package.name()
            );
            assert!(paths.contains(&ENTRYPOINT_PATH), "{}", package.name());

            // Typst's package resolver reads these keys, so a manifest that spells any of them
            // differently fails an install rather than this crate.
            let manifest = package
                .file(Path::new(MANIFEST_PATH))
                .expect("every package carries a manifest");
            assert!(
                manifest.contains(&format!("name = \"{}\"", package.name())),
                "{} names itself: {manifest}",
                package.name()
            );
            assert!(
                manifest.contains(&format!("version = \"{TOLA_PACKAGE_VERSION}\"")),
                "{} carries the fixed import version: {manifest}",
                package.name()
            );
            assert!(
                manifest.contains(&format!("entrypoint = \"{ENTRYPOINT_PATH}\"")),
                "{} points at its entrypoint: {manifest}",
                package.name()
            );
        }
    }

    #[test]
    fn host_is_the_only_engine_only_package() {
        let author_facing = builtin_packages()
            .map(|package| package.package)
            .collect::<Vec<_>>();
        assert_eq!(
            author_facing,
            vec![
                TolaPackage::Site,
                TolaPackage::Address,
                TolaPackage::Icon,
                TolaPackage::Image,
                TolaPackage::Source,
                TolaPackage::Collection,
                TolaPackage::Schema,
                TolaPackage::Document,
                TolaPackage::Code,
                TolaPackage::Web,
            ]
        );
        assert_eq!(
            resolvable_packages().count(),
            TolaPackage::all().len(),
            "the compiler resolves every builtin package"
        );
    }

    #[test]
    fn package_resolves_from_its_own_spec() {
        for package in TolaPackage::all().iter().copied() {
            assert_eq!(TolaPackage::from_spec(&package.spec()), Some(package));
        }
    }

    #[test]
    fn entrypoint_exports_follow_its_own_statements() {
        let exports = export_names(
            r#"#let single = 1
#let (first, second) = (1, 2)
#let named(value) = value
#import "@tola/host:0.0.0": imported, renamed as alias
#import "keys.typ": *
#let leaked = { import "keys.typ": hidden; hidden }
#let single = 2
"#,
        );
        assert_eq!(
            exports,
            [
                "single", "first", "second", "named", "imported", "alias", "leaked",
            ]
        );
    }

    /// The names each real entrypoint publishes, as the syntax-tree discovery reads them: the
    /// wildcard `@tola/host` imports publishes nothing, and its empty list records that.
    ///
    /// A discovery that stops reading one of the forms an entrypoint uses — a parenthesized
    /// import list, say — would otherwise drop that name silently, while every description check
    /// that names whatever it was told about still passes.
    #[test]
    fn package_exports_match_their_entrypoints() {
        let expected: [(&str, &[&str]); 11] = [
            ("host", &[]),
            ("site", &["site"]),
            (
                "address",
                &[
                    "slugify",
                    "route",
                    "route-to-output",
                    "output-to-route",
                    "output-to-url",
                    "decode-url-path",
                    "asset-url",
                ],
            ),
            ("icon", &["icon", "icon-bytes", "icon-url"]),
            ("image", &["resize-image", "image-metadata"]),
            (
                "source",
                &[
                    "all-sources",
                    "current-source",
                    "tola-meta",
                    "parse-sources",
                ],
            ),
            (
                "collection",
                &[
                    "pick",
                    "index-by",
                    "group-by",
                    "group-by-keys",
                    "select-members",
                    "adjacent",
                    "window",
                    "children",
                    "descendants",
                    "ancestors",
                    "lineage",
                    "siblings",
                ],
            ),
            (
                "schema",
                &[
                    "any",
                    "schema",
                    "optional",
                    "nullable",
                    "literal",
                    "enum-of",
                    "array-of",
                    "dictionary-of",
                    "one-or-many",
                    "tuple",
                    "lazy",
                    "union",
                    "variant",
                    "trim",
                    "non-empty",
                    "refine",
                    "check",
                    "map",
                    "convert",
                    "describe",
                    "inspect",
                    "parse",
                    "try-parse",
                    "ok",
                    "err",
                    "issue",
                    "format-issues",
                    "min-length",
                    "max-length",
                    "matches",
                    "at-least",
                    "at-most",
                    "email",
                    "ipv4",
                    "ipv6",
                    "ip",
                    "http-url",
                    "https-url",
                ],
            ),
            ("document", &["current-document", "references", "headings"]),
            (
                "code",
                &[
                    "code-themes",
                    "render-code",
                    "code-stylesheet-url",
                    "code-stylesheet",
                ],
            ),
            (
                "web",
                &[
                    "plain-text",
                    "head-metadata",
                    "canonical",
                    "open-graph",
                    "twitter-card",
                    "math-svg",
                    "feed",
                    "sitemap",
                ],
            ),
        ];
        assert_eq!(expected.len(), TolaPackage::all().len());
        for (name, expected) in expected {
            let package = TolaPackage::all()
                .iter()
                .copied()
                .find(|package| package.name() == name)
                .unwrap_or_else(|| panic!("no builtin package is named `{name}`"));
            assert_eq!(
                package.exports().collect::<std::collections::BTreeSet<_>>(),
                expected
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>(),
                "@tola/{name} publishes"
            );
        }
    }

    #[test]
    fn source_names_match_the_builtin_package() {
        assert_eq!(TolaPackage::Source.name(), SOURCE_PACKAGE);
        let exports = TolaPackage::Source.exports().collect::<Vec<_>>();
        for name in [ALL_SOURCES, CURRENT_SOURCE, PARSE_SOURCES, TOLA_META] {
            assert!(
                exports.contains(&name),
                "`@tola/source` does not publish `{name}`"
            );
        }
    }

    /// The names one package source documents, each with the first line of the `///` block directly
    /// above the statement that binds it: the hover text the language service reads for that name.
    fn documented_names(module: &'static str) -> Vec<(&'static str, &'static str)> {
        let source = Source::detached(module);
        let root = LinkedNode::new(source.root());
        let lines = module.lines().collect::<Vec<_>>();
        let mut documented = Vec::new();
        for statement in root.children() {
            let Some(line) = source.lines().byte_to_line(statement.range().start) else {
                continue;
            };
            let mut first = None;
            for above in (0..line).rev() {
                let Some(text) = lines.get(above) else { break };
                let Some(comment) = text.trim_start().strip_prefix("///") else {
                    break;
                };
                first = Some(comment.trim());
            }
            let Some(first) = first else { continue };
            let mut names = Vec::new();
            if let Some(binding) = statement.cast::<ast::LetBinding>() {
                for name in binding.kind().bindings() {
                    publish(&mut names, module, &statement, name);
                }
            } else if let Some(import) = statement.cast::<ast::ModuleImport>()
                && let Some(ast::Imports::Items(items)) = import.imports()
            {
                for item in items.iter() {
                    publish(&mut names, module, &statement, item.bound_name());
                }
            }
            documented.extend(names.into_iter().map(|name| (name, first)));
        }
        documented
    }

    /// Every name a package publishes documents itself: a `///` block directly above the statement
    /// that binds it, opening with one sentence.
    #[test]
    fn every_published_name_documents_itself() {
        for package in TolaPackage::all() {
            let documented = package
                .definition()
                .sources
                .iter()
                .filter(|source| source.path.ends_with(".typ"))
                .flat_map(|source| documented_names(source.contents))
                .collect::<std::collections::BTreeMap<_, _>>();
            for name in package.exports() {
                let first = documented.get(name).unwrap_or_else(|| {
                    panic!(
                        "@tola/{} publishes `{name}` without hover docs",
                        package.name()
                    )
                });
                assert!(
                    first.ends_with('.') && !first.contains(". "),
                    "`{name}` opens its hover docs with one sentence: `{first}`"
                );
            }
        }
    }

    #[test]
    fn foreign_namespace_names_no_builtin() {
        assert_eq!(
            TolaPackage::from_spec(&PackageSpec {
                namespace: "other".into(),
                name: "site".into(),
                version: TOLA_PACKAGE_VERSION,
            }),
            None
        );
    }

    /// Tola's own Typst surface: lowercase words joined by `-`, never `_`.
    ///
    /// The check reads the real surface: the exports each entrypoint's syntax tree publishes, the
    /// parameters of every exported function, and the fixed field names the package natives
    /// insert. Host-language identifiers, upstream protocol names, and site data are not Tola's
    /// to spell.
    #[test]
    fn typst_names_use_hyphenated_spelling() {
        for package in TolaPackage::all().iter().copied() {
            let builtin = builtin_package(&package.spec()).expect("every builtin package resolves");
            for name in builtin.exports() {
                assert!(
                    hyphenated(name),
                    "@tola/{} publishes `{name}` without hyphenated spelling",
                    package.name()
                );
                let documentation = builtin
                    .export_documentation(&[name.to_owned()])
                    .unwrap_or_else(|error| {
                        panic!("@tola/{} documents `{name}`: {error}", package.name())
                    });
                for export in documentation {
                    if let crate::docs::ExportDeclaration::Function(function) = export.declaration {
                        for parameter in function.parameters {
                            assert!(
                                hyphenated(&parameter.name),
                                "@tola/{} `{name}` parameter `{}` is not hyphenated",
                                package.name(),
                                parameter.name
                            );
                        }
                    }
                }
            }
        }
        for (api, fields) in [
            ("image-metadata", &crate::images::IMAGE_METADATA_FIELDS[..]),
            ("resize-image", &crate::images::RESIZE_IMAGE_FIELDS[..]),
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

    /// Whether `name` is lowercase words joined by single hyphens.
    fn hyphenated(name: &str) -> bool {
        let mut words = name.split('-');
        let Some(first) = words.next() else {
            return false;
        };
        let word = |word: &str| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        };
        word(first) && words.all(word)
    }
}
