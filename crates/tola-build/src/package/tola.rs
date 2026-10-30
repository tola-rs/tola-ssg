//! The package observations the engine produces and reads back.
//!
//! The packages themselves, their sources, and their natives belong to `tola-packages`. What
//! stays here is the producer side: the bytes a package path has in one compilation, and the
//! lexical source a `current-source()` read names.

use std::path::Path;

use tola_packages::TolaPackage;
use tola_typst::prelude::*;
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

/// The bytes one package path holds in this compilation, or `None` when the package is not a
/// Tola package or the path names nothing in it. Observation descriptors and request paths
/// resolve to empty bytes, because their own path is the whole observation.
pub(crate) fn read_package(pkg: &PackageSpec, path: &str) -> Option<Vec<u8>> {
    let tola_pkg = TolaPackage::from_spec(pkg)?;
    let path = path.strip_prefix('/').unwrap_or(path);
    if tola_pkg == TolaPackage::Image
        && let Some(bytes) = tola_packages::observation_bytes(pkg, path)
    {
        return Some(bytes);
    }
    if tola_pkg.is_observation_descriptor(Path::new(path)) {
        return Some(Vec::new());
    }
    if tola_packages::published_icon_request(pkg, Path::new(path)).is_some() {
        // A request is named by its own path, so its observation has no content.
        return Some(Vec::new());
    }
    tola_pkg
        .file(Path::new(path))
        .map(|contents| contents.into_owned().into_bytes())
}

/// The target one icon package path answers a read with, from the collections a host bound: a
/// configured icon's normalized SVG, or the tracked name list of one configured collection.
///
/// Any other path, and a name the collections do not carry, is missing, which leaves the miss to
/// the read that asked for it.
pub(crate) fn icon_file_target(
    collections: &tola_icons::IconCollections,
    package: &PackageSpec,
    path: &Path,
) -> tola_typst::FileTarget {
    tola_packages::icon_file_bytes(collections, package, path)
        .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
        .or_else(|| {
            tola_packages::icon_collection_names_bytes(collections, package, path)
                .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
        })
        .unwrap_or(tola_typst::FileTarget::Missing)
}

/// Whether this package path's bytes are fixed for the process.
///
/// Only Tola's own package files, immutable observation descriptors, and published icon request
/// paths — whose observation is their own name — qualify; a package's dynamic files stay
/// unstable, so naming the package is never enough.
pub(crate) fn package_file_is_process_stable(package: &PackageSpec, path: &Path) -> bool {
    let Some(kind) = TolaPackage::from_spec(package) else {
        return false;
    };
    kind.is_observation_descriptor(path)
        || kind.file(path).is_some()
        || tola_packages::published_icon_request(package, path).is_some()
        || path
            .to_str()
            .is_some_and(|path| tola_packages::observation_bytes(package, path).is_some())
}

/// The content source a lexical `current-source()` call reads.
pub(crate) fn source_query_file(locator: &ReadLocator) -> Option<RootedPath> {
    let (ReadLocator::Package { package, path } | ReadLocator::ProvidedPackage { package, path }) =
        locator
    else {
        return None;
    };
    let relative = TolaPackage::from_spec(package)?.observed_source_path(path)?;
    let file = VirtualPath::virtualize(Path::new(""), relative).ok()?;
    Some(RootedPath::new(VirtualRoot::Project, file))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::content::{
        ContentId, ContentSourceLayout, ContentUnit, SourceMetadataDeclaration, SourceRecords,
        SourceSet,
    };
    use crate::metadata::SourceMetadata;
    use crate::package::SiteBindings;
    use tempfile::TempDir;
    use typst::World;
    use typst::foundations::{Array, Dict, IntoValue};

    #[derive(Default)]
    struct IconFiles(Arc<tola_icons::IconCollections>);

    impl tola_typst::FileProvider for IconFiles {
        fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
            let typst::syntax::VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
            if tola_packages::is_icon_file(package, path) {
                return Some(icon_file_target(&self.0, package, path));
            }
            read_package(package, id.vpath().get_with_slash())
                .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
        }
    }

    /// A world compiling `source` against `library` and `icons`, with `fonts` when given.
    fn typst_world(
        source: &str,
        library: &tola_packages::library::SiteLibrary,
        icons: Arc<tola_icons::IconCollections>,
        fonts: Option<Arc<FontStore>>,
    ) -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, source).unwrap();
        let files = tola_typst::FileResolver::new().with_provider(IconFiles(icons));
        let builder = TypstWorld::builder(&main, directory.path())
            .with_files(Arc::new(files))
            .with_local_cache()
            .with_shared_library(library.shared());
        let builder = match fonts {
            Some(fonts) => builder.with_fonts(fonts),
            None => builder.no_fonts(),
        };
        let world = builder
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid test world");
        (directory, world)
    }

    /// A world compiling against the given library and icon collections.
    fn world_with_icons(
        source: &str,
        library: &tola_packages::library::SiteLibrary,
        icons: Arc<tola_icons::IconCollections>,
    ) -> (TempDir, TypstWorld) {
        typst_world(source, library, icons, None)
    }

    /// The library of a default site configuration holding `sources`.
    fn default_library(sources: SourceRecords) -> tola_packages::library::SiteLibrary {
        let site_config = crate::config::tests::OwnedSiteConfig::new("");
        let identity = SiteBindings::from_config(&site_config.config, Default::default());
        identity.library(sources)
    }

    /// A world compiling against the library of a default site configuration.
    fn world_from_default_config(source: &str, sources: SourceRecords) -> (TempDir, TypstWorld) {
        world_with_icons(source, &default_library(sources), Arc::default())
    }

    /// The compilation of `source` in a default site world with no source records.
    fn compiled_from_default_config(source: &str) -> (TempDir, tola_typst::BundleCompilation) {
        let (directory, world) = world_from_default_config(source, SourceRecords::default());
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        (directory, compilation)
    }

    /// Asserts a default site world compiles `source`.
    fn assert_default_compiles(source: &str) {
        let _ = compiled_from_default_config(source);
    }

    /// The example site's configuration: one published asset tree and one icon collection.
    const EXAMPLE_SITE_CONFIG: &str = r#"[site]
title = "Example site"

[assets]
[[assets.trees]]
source = "assets"
url-prefix = "/assets"

[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
"#;

    /// The SVG of the example site's `mark` asset and `brand:mark` icon.
    const MARK_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M12 2 2 22h20z"/></svg>"#;

    /// The SVG of the example site's `brand:logo` icon.
    const LOGO_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="currentColor"/></svg>"#;

    /// The example site's `assets/logo.png`: 8×4 opaque pixels.
    fn logo_png() -> Vec<u8> {
        let pixels = ::image::RgbImage::from_pixel(8, 4, ::image::Rgb([0x33, 0x66, 0x99]));
        let mut encoded = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    /// The host library of a world that only evaluates a source, with no site records.
    fn source_library() -> Arc<typst::utils::LazyHash<typst::Library>> {
        tola_packages::library::SiteLibrary::new(tola_packages::library::HostInputs {
            site: Dict::new(),
            asset_urls: Dict::new(),
            asset_origins: Dict::new(),
            source_records: Array::new(),
            source_records_by_file: Dict::new(),
            source_origins: Dict::new(),
        })
        .shared()
    }

    /// The metadata declaration `source` has, scanned in a world that binds no site records.
    ///
    /// `None` when the source declares none.
    fn declared_metadata(source: &Path, root: &Path) -> Option<SourceMetadataDeclaration> {
        let world = TypstWorld::builder(source, root)
            .with_files(Arc::new(
                tola_typst::FileResolver::new().with_provider(IconFiles::default()),
            ))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(source_library())
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid source world");
        let header = tola_packages::CaptureHeader::new(world.main(), "metadata");
        let (captured, scanned) =
            tola_typst::scan_world_observed(&world, [header.value()]).into_parts();
        let scanned = scanned.unwrap();
        match tola_packages::decode_capture(&captured, &header, scanned.source())
            .unwrap()
            .declaration
        {
            tola_packages::Declaration::One(declared) => Some(SourceMetadataDeclaration {
                metadata: SourceMetadata::from_dict(declared.metadata),
                range: declared.range,
            }),
            _ => None,
        }
    }

    /// The example site's content sources, in discovery order: the path below `content/` and the
    /// source's own text.
    const EXAMPLE_CONTENT: [(&str, &str); 7] = [
        (
            "index.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Welcome\"))\n",
        ),
        (
            "guide/index.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Guide\", order: 0))\n",
        ),
        (
            "guide/install.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Install\", order: 1))\n",
        ),
        (
            "guide/deploy.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Deploy\", order: 2))\n",
        ),
        (
            "guide/advanced/plugins.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Plugins\", order: 3))\n",
        ),
        (
            "notes/first-post.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"First post\", tags: (\"web\",), \
             published: datetime(year: 2026, month: 5, day: 1)))\n",
        ),
        (
            "notes/second-post.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((title: \"Second post\", tags: (\"web\", \"typst\"), \
             published: datetime(year: 2026, month: 6, day: 1)))\n",
        ),
    ];

    /// A world compiling `source` against the example site a `site` fence selects.
    ///
    /// Authors may rely on exactly these inputs:
    /// - `content/` holds seven sources, in this discovery order: `index.typ` (title `Welcome`),
    ///   `guide/index.typ` (title `Guide`, `order` 0), `guide/install.typ` (title `Install`,
    ///   `order` 1), `guide/deploy.typ` (title `Deploy`, `order` 2),
    ///   `guide/advanced/plugins.typ` (title `Plugins`, `order` 3), `notes/first-post.typ`
    ///   (title `First post`, tags `("web",)`, published 2026-05-01), and
    ///   `notes/second-post.typ` (title `Second post`, tags `("web", "typst")`, published
    ///   2026-06-01).
    /// - `assets/` is published below `/assets` and holds `logo.png`, an 8×4 opaque PNG, and
    ///   `mark.svg`.
    /// - `brand` is an icon collection with the icons `brand:mark` and `brand:logo`.
    /// - `site.base-path` is `/` and `site.title` is `Example site`.
    /// - The world has Tola's embedded fonts and no system fonts, so an example that lays
    ///   content out — a math frame from `@tola/web`, for instance — renders offline.
    ///
    /// `source` becomes the site root's `main.typ`, so a relative path such as `assets/logo.png`
    /// reads from the site root. The example's own file is not a content source, so
    /// `current-source()` names no record here.
    fn world_from_example_site(source: &str) -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), EXAMPLE_SITE_CONFIG);
        let root = config.get_root();

        let content = root.join("content");
        let mut units = Vec::new();
        let mut declarations = BTreeMap::new();
        for (path, text) in EXAMPLE_CONTENT {
            let source = content.join(path);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, text).unwrap();
            declarations.insert(source.clone(), declared_metadata(&source, root));
            units.push(ContentUnit {
                root: content.clone(),
                id: ContentId::new(path.into()),
                source,
                layout: ContentSourceLayout::from_entry_path(Path::new(path)),
            });
        }

        let assets = root.join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("logo.png"), logo_png()).unwrap();
        fs::write(assets.join("mark.svg"), MARK_SVG).unwrap();

        let icons = root.join("icons/brand");
        fs::create_dir_all(&icons).unwrap();
        fs::write(icons.join("mark.svg"), MARK_SVG).unwrap();
        fs::write(icons.join("logo.svg"), LOGO_SVG).unwrap();

        let cancellation = crate::cancellation::BuildCancellation::default();
        let collections = crate::icon::prepare(
            &config,
            &cancellation,
            None,
            &crate::resources::BuildResources::default(),
            &crate::filesystem::SourceOverrides::default(),
        )
        .expect("the example site's icon collection loads")
        .collections();
        let asset_urls = crate::asset::render_configured_asset_inventory(
            &config,
            &crate::resources::source_boundary(&config, crate::InputScope::Online),
            &cancellation,
            None,
            &[],
        )
        .expect("the example site's asset tree renders")
        .asset_urls(&config)
        .expect("the example site's asset URLs resolve");
        let records = SourceSet::without_metadata(&units, &config)
            .unwrap()
            .with_metadata(&declarations)
            .inputs()
            .to_source_records();
        let library = SiteBindings::from_config(&config, asset_urls).library(records);

        let main = root.join("main.typ");
        fs::write(&main, source).unwrap();
        let files = tola_typst::FileResolver::new().with_provider(IconFiles(collections));
        let fonts = Arc::new(FontStore::with_options(
            FontOptions::new()
                .with_system_fonts(false)
                .with_embedded_fonts(true),
        ));
        let world = TypstWorld::builder(&main, root)
            .with_files(Arc::new(files))
            .with_local_cache()
            .with_shared_library(library.shared())
            .with_fonts(fonts)
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid example site world");
        (directory, world)
    }

    /// Which world one documented example compiles in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ExampleWorld {
        /// A default site configuration, with no sources, assets, or icons.
        Default,
        /// The example site `world_from_example_site` builds.
        ExampleSite,
    }

    impl ExampleWorld {
        /// The world's name in a failure message.
        fn name(self) -> &'static str {
            match self {
                Self::Default => "default",
                Self::ExampleSite => "example site",
            }
        }
    }

    /// The examples `docs` has: the documentation line each fence opens at, the fence body
    /// verbatim, and the world it compiles in.
    ///
    /// A fence's info string is `typ`, `typc`, or `typst`, optionally followed by `site`, which
    /// selects the example site. Any other info string is an error naming its line.
    fn documented_examples(docs: &str) -> Result<Vec<(usize, String, ExampleWorld)>, String> {
        let mut examples = Vec::new();
        let mut fence = None;
        let mut source = String::new();
        for (index, line) in docs.lines().enumerate() {
            let trimmed = line.trim_start();
            let marker = trimmed.as_bytes().first().copied();
            let length = trimmed
                .bytes()
                .take_while(|byte| Some(*byte) == marker)
                .count();
            let is_fence = matches!(marker, Some(b'`' | b'~')) && length >= 3;
            if let Some((opening, minimum, start, world)) = fence {
                if is_fence
                    && marker == opening
                    && length >= minimum
                    && trimmed[length..].trim().is_empty()
                {
                    examples.push((start, std::mem::take(&mut source), world));
                    fence = None;
                } else {
                    source.push_str(line);
                    source.push('\n');
                }
            } else if is_fence {
                let info = trimmed[length..].trim();
                let world = match info.split_whitespace().collect::<Vec<_>>().as_slice() {
                    ["typ" | "typst" | "typc"] => ExampleWorld::Default,
                    ["typ" | "typst" | "typc", "site"] => ExampleWorld::ExampleSite,
                    _ => {
                        return Err(format!(
                            "API examples must use a `typ`, `typc`, or `typst` fence, optionally \
                             followed by `site`, at documentation line {}; found `{info}`",
                            index + 1
                        ));
                    }
                };
                fence = Some((marker, length, index + 1, world));
            }
        }
        if let Some((_, _, start, _)) = fence {
            return Err(format!(
                "unclosed API example fence at documentation line {start}"
            ));
        }
        Ok(examples)
    }

    #[test]
    fn documented_examples_compile() {
        let mut checked = 0;
        for package in tola_packages::builtin_packages() {
            let names = package.exports().map(str::to_owned).collect::<Vec<_>>();
            let mut sections = vec![("overview".to_owned(), package.overview())];
            sections.extend(
                package
                    .export_documentation(&names)
                    .unwrap()
                    .into_iter()
                    .map(|export| {
                        let summary = export.documentation.summary.clone();
                        (export.name, (!summary.is_empty()).then_some(summary))
                    }),
            );
            for (name, docs) in sections {
                let Some(docs) = docs else { continue };
                let examples = documented_examples(&docs)
                    .unwrap_or_else(|error| panic!("{} / {name}: {error}", package.spec()));
                for (index, (line, source, selected)) in examples.into_iter().enumerate() {
                    // Compile exactly what the author can copy: injected imports or document
                    // wrappers would conceal missing dependencies and invalid placement.
                    let (_directory, world) = match selected {
                        ExampleWorld::Default => {
                            world_from_default_config(&source, SourceRecords::default())
                        }
                        ExampleWorld::ExampleSite => world_from_example_site(&source),
                    };
                    let compilation = tola_typst::compile_bundle_world(&world, &Default::default());
                    assert!(
                        compilation.is_ok(),
                        "{} / {name} / example {} (documentation line {line}) in the {} world\n\
                         {source}\n{}",
                        package.spec(),
                        index + 1,
                        selected.name(),
                        compilation.unwrap_err(),
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "no public API examples were discovered");
    }

    #[test]
    fn fenced_examples_keep_their_text() {
        let docs = "Example:\n````typst\n#let sample = \"```\"\n  #assert.eq(sample, \"```\")\n````\n\n~~~typ\n#assert(true)\n~~~\n\n~~~typc\n#assert(true)\n~~~";
        assert_eq!(
            documented_examples(docs).unwrap(),
            vec![
                (
                    2,
                    "#let sample = \"```\"\n  #assert.eq(sample, \"```\")\n".into(),
                    ExampleWorld::Default,
                ),
                (7, "#assert(true)\n".into(), ExampleWorld::Default),
                (11, "#assert(true)\n".into(), ExampleWorld::Default),
            ]
        );
        for malformed in ["```typst\n#assert(true)", "```tysp\n#assert(true)\n```"] {
            assert!(documented_examples(malformed).is_err());
        }
    }

    #[test]
    fn example_site_provides_its_inputs() {
        let docs = r#"Example:
```typst site
#import "@tola/image:0.0.0": image-metadata
#import "@tola/icon:0.0.0": icon-bytes
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources
#assert.eq(image-metadata("assets/logo.png").width, 8)
#assert.eq(all-sources().first().id, "index.typ")
#assert.eq(all-sources().first().path, "index.typ")
#assert.eq(all-sources().first().filename, "index.typ")
#assert.eq(all-sources().first().meta, (title: "Welcome"))
#assert.eq(all-sources().last().route-segments, ("notes", "second-post"))
#assert.eq(type(icon-bytes("brand:mark")), bytes)
#assert.eq(type(icon-bytes("brand:logo")), bytes)
#assert.eq(site.base-path, "/")
#assert.eq(site.title, "Example site")
```
"#;
        let examples = documented_examples(docs).unwrap();
        let [(line, source, selected)] = examples.as_slice() else {
            panic!("one example fence: {examples:?}");
        };
        assert_eq!(*line, 2);
        assert_eq!(*selected, ExampleWorld::ExampleSite);
        let (_directory, world) = world_from_example_site(source);
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect("the example site provides every input the example reads");
    }

    /// A program importing every `@tola/<package>` native and running `assertions` before its one
    /// document.
    fn package_program(package: &str, assertions: &str) -> String {
        format!(
            "#import \"@tola/{package}:0.0.0\": *\n\
             {assertions}\n\
             #document(\"assertions.html\")[]"
        )
    }

    /// The world package assertions run in: the host library with no site records, so a package
    /// can use the host natives it imports without receiving site data.
    fn package_world(source: &str) -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, source).unwrap();
        let files = tola_typst::FileResolver::new().with_provider(IconFiles::default());
        let world = TypstWorld::builder(&main, directory.path())
            .with_files(Arc::new(files))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(source_library())
            .build(&tola_typst::BundleCancellation::default())
            .expect("valid test world");
        (directory, world)
    }

    /// Compiles `assertions` against `@tola/<package>` in a world with no site records, which must
    /// hold.
    fn assert_package(package: &str, assertions: &str) {
        let (_directory, world) = package_world(&package_program(package, assertions));
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect("package assertions must hold");
    }

    /// Asserts compiling `assertions` against `@tola/<package>` fails with diagnostics.
    fn reject_package(package: &str, assertions: &str) {
        let (_directory, world) = package_world(&package_program(package, assertions));
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect_err("invalid assertions must fail compiling")
            .diagnostics()
            .expect("Typst failure has diagnostics");
    }

    fn source_records(ids: &[&str]) -> SourceRecords {
        let site_config = crate::config::tests::OwnedSiteConfig::new("");
        let root = site_config.config.get_root().join("content");
        let units = ids
            .iter()
            .map(|id| ContentUnit {
                root: root.clone(),
                id: ContentId::new((*id).into()),
                source: root.join(id),
                layout: ContentSourceLayout::from_entry_path(Path::new(id)),
            })
            .collect::<Vec<_>>();
        SourceSet::without_metadata(&units, &site_config.config)
            .unwrap()
            .inputs()
            .to_source_records()
    }

    #[test]
    fn address_errors_point_at_argument() {
        for (function, argument) in [
            ("output-of", "relative"),
            ("route-of", "/index.html"),
            ("decode-url-path", "/%zz"),
        ] {
            let source = format!(
                "#import \"@tola/address:0.0.0\": {function}\n\
                 #let url = {function}({argument:?})\n\
                 #document(\"index.html\")[]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(error.location.path.as_deref(), Some("main.typ"));
            let range = error.location.range.unwrap();
            let line = source.lines().nth(range.start.line).unwrap();
            assert_eq!(range.start.line, range.end.line);
            assert_eq!(
                line[range.start.character..range.end.character].trim_matches('"'),
                argument,
                "{error:?}"
            );
        }
    }

    #[test]
    fn package_helper_failure_keeps_the_call_site() {
        for (helper, is_typst_error) in [
            (
                r#"#let url() = address("/guide/", base-path: "/docs")"#,
                false,
            ),
            (r#"#let url() = address(panic("route unavailable"))"#, true),
        ] {
            let source = format!(
                "#import \"@tola/address:0.0.0\": route-url as address\n\
                 {helper}\n\
                 #let selected = url()\n\
                 #document(\"index.html\")[]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(error.location.path.as_deref(), Some("main.typ"));
            assert_eq!(error.location.line, Some(2));
            assert_eq!(
                error.origin == tola_typst::DiagnosticOrigin::Typst,
                is_typst_error
            );
            assert!(
                error.traces.iter().any(|trace| {
                    trace.location.path.as_deref() == Some("main.typ")
                        && trace.location.line == Some(3)
                }),
                "{error:?}"
            );
        }
    }

    #[test]
    fn url_path_schema_decodes_once() {
        assert_default_compiles(
            r#"#import "@tola/schema:0.0.0": parse, try-parse, schema, url-path
#import "@tola/address:0.0.0": route-url
#let route = parse("/文档/Hello World/a%2523b/", url-path)
#assert.eq(route, "/文档/Hello World/a%23b/")
#assert.eq(route-url(route, base-path: "/docs/"), "/docs/%E6%96%87%E6%A1%A3/Hello%20World/a%2523b/")
#assert.eq(parse("/a%23b/", url-path), "/a#b/")
#let page = schema((permalink: url-path))
#for path in ("/bad%/", "/a%2Fb/", "/guide /") {
  let rejected = try-parse((permalink: path), page)
  assert.eq(rejected.ok, false)
  assert.eq(rejected.issues.at(0).code, "schema.url-path")
  assert.eq(rejected.issues.at(0).path, ("permalink",))
}
#document("index.html")[Paths]"#,
        );
    }

    #[test]
    fn helper_sees_its_own_source() {
        let sources = source_records(&["helper.typ"]);
        let (directory, world) = world_from_default_config(
            r#"
#import "content/helper.typ": helper-source
#assert.eq(helper-source().filename, "helper.typ")
#assert.eq(helper-source().path, "helper.typ")
#document("index.html")[]
"#,
            sources,
        );
        fs::create_dir(directory.path().join("content")).unwrap();
        fs::write(
            directory.path().join("content/helper.typ"),
            r#"
#import "@tola/source:0.0.0": current-source
#let helper-source() = current-source()
"#,
        )
        .unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        assert_eq!(compilation.documents().count(), 1);
    }

    #[test]
    fn included_source_keeps_its_own_identity() {
        let sources = source_records(&["document.typ"]);
        let (directory, world) = world_from_default_config(
            r#"
#import "@tola/source:0.0.0": all-sources
#let document-source = all-sources().first()
#document("first/index.html")[
  #include document-source.file
]
#document("second.html")[
  #include document-source.file
]
"#,
            sources,
        );
        fs::create_dir(directory.path().join("content")).unwrap();
        fs::write(
            directory.path().join("content/document.typ"),
            r#"
#import "@tola/address:0.0.0": join-route
#import "@tola/document:0.0.0": current-document
#import "@tola/source:0.0.0": current-source
#let source = current-source()
#assert.eq(source.path, "document.typ")
#assert.eq(join-route(source.at("route-segments")), "/document/")
#import "@tola/source:0.0.0": tola-meta
#tola-meta((filename: source.filename))
#context {
  let page = current-document()
  assert.eq(source.id, "document.typ")
  assert.eq(source.file, path("document.typ"))
  assert(page.output in ("first/index.html", "second.html"))
}
"#,
        )
        .unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        assert_eq!(compilation.documents().count(), 2);
    }

    #[test]
    fn qualified_imports_reach_their_own_package() {
        let sources = source_records(&["document.typ"]);
        let (directory, world) = world_from_default_config(
            r#"
#import "@tola/source:0.0.0"
#import "@tola/web:0.0.0"
#import "@tola/document:0.0.0" as doc
#let document-source = source.all-sources().first()
#assert.eq(document-source.filename, "document.typ")
#assert.eq(web.plain-text([Native *title*]), "Native title")
#assert.eq(web.head-metadata((:)).len(), 2)
#document("first/index.html")[#include document-source.file]
#document("second.html")[#include document-source.file]
#document("native.html")[
  #html.html[
    #html.head(web.head-metadata((title: "Native title")).join())
    #html.body[#context assert.eq(doc.current-document().output, "native.html")]
  ]
]
"#,
            sources,
        );
        fs::create_dir(directory.path().join("content")).unwrap();
        fs::write(
            directory.path().join("content/document.typ"),
            r#"
#import "@tola/document:0.0.0": current-document
#import "@tola/source:0.0.0": current-source
#let source = current-source()
#context {
  let page = current-document()
  assert.eq(source.id, "document.typ")
  assert.eq(source.file, path("document.typ"))
  assert(page.output in ("first/index.html", "second.html"))
}
"#,
        )
        .unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        assert_eq!(compilation.documents().count(), 3);
    }

    #[test]
    fn graph_preserves_identity() {
        assert_package(
            "graph",
            r##"
#let nodes = ((id: "b", payload: none), (id: "a", payload: [rich]), (id: "", payload: 9), (id: "isolated", payload: none))
#let links = ((from: "a", to: "b", payload: [one]), (from: "a", to: "b", payload: [one]), (from: "b", to: "b", payload: none), (from: "b", to: "", payload: 4))
#let g = from-records(nodes, links, node-key: n => n.id, source: e => e.from, target: e => e.to)
#assert.eq(node-keys(g), ("b", "a", "", "isolated"))
#assert(has-node(g, "isolated"))
#assert(not has-node(g, "missing"))
#assert.eq(node-data(g, "isolated").payload, none)
#assert.eq(node-data(g, "a").payload, [rich])
#assert.eq(edges(g).map(e => e.index), (0, 1, 2, 3))
#assert.eq(edges(g).at(0).data, links.at(0))
#assert.eq(out-edges(g, "a").map(e => e.index), (0, 1))
#assert.eq(in-edges(g, "b").map(e => e.index), (0, 1, 2))
#assert.eq(out-edges(g, "b").map(e => e.index), (2, 3))
#assert.eq(in-edges(g, "isolated"), ())
#let chosen = select-edges(g, e => e.index == 1 or e.index == 3)
#assert.eq(node-keys(chosen), node-keys(g))
#assert.eq(edges(chosen).map(e => e.index), (1, 3))
#assert.eq(out-edges(chosen, "a").map(e => e.index), (1,))
#let sub = induced(g, ("", "b", "b"))
#assert.eq(node-keys(sub), ("b", ""))
#assert.eq(edges(sub).map(e => e.index), (2, 3))
#assert.eq(edges(g).len(), 4)
#let changed = edges(g)
#changed.at(0).insert("source", "changed")
#assert.eq(edges(g).at(0).source, "a")
#let rich = from-records((none,), ([edge],), node-key: _ => "n", source: _ => "n", target: _ => "n")
#assert(has-node(rich, "n"))
#assert.eq(node-data(rich, "n"), none)
#assert.eq(edges(rich).at(0).data, [edge])
#assert.eq(edges(select-edges(rich, _ => true)).at(0).data, [edge])
#let keys = ("A", "a", "é", "é")
#let exact = from-records(keys, ())
#assert.eq(node-keys(exact), keys)
#let callable = from-records((x => x + 1,), (x => x + 2,), node-key: _ => "f", source: _ => "f", target: _ => "f")
#let node-fn = node-data(callable, "f")
#let edge-fn = edges(induced(callable, ("f",))).at(0).data
#assert.eq(node-fn(1), 2)
#assert.eq(edge-fn(1), 3)
"##,
        );
    }

    #[test]
    fn graph_reachability_includes_roots() {
        assert_package(
            "graph",
            r##"
#let g = from-records(("b", "a", "c", "isolated"), ((source: "a", target: "b"), (source: "b", target: "a"), (source: "b", target: "c"), (source: "c", target: "c")))
#assert.eq(reachable(g, ("a",)), ("b", "a", "c"))
#assert.eq(reachable(g, ("a", "b", "a")), ("b", "a", "c"))
#assert.eq(reachable(g, ("c",), direction: "in"), ("b", "a", "c"))
#assert.eq(reachable(g, ("isolated",)), ("isolated",))
#assert.eq(reachable(g, ()), ())
#let chain = from-records(("a", "b"), ((source: "a", target: "b"),))
#assert.eq(reachable(chain, ("a",)), ("a", "b"))
#assert.eq(reachable(chain, ("a", "b")), ("a", "b"))
"##,
        );
    }

    #[test]
    fn graph_rejects_invalid_values() {
        for expression in [
            "from-records(none, ())",
            "from-records((), none)",
            "from-records((1,), ())",
            r#"from-records(("a", "a"), ())"#,
            r#"from-records(("a",), ((source: "missing", target: "a"),))"#,
            r#"from-records(("a",), ((source: "a", target: "missing"),))"#,
            r#"from-records(("a",), ((source: 1, target: "a"),))"#,
            r#"from-records(("a",), ((source: "a", target: none),))"#,
            r#"has-node(from-records((), ()), 1)"#,
            r#"node-data(from-records((), ()), "missing")"#,
            r#"in-edges(from-records((), ()), "missing")"#,
            r#"out-edges(from-records((), ()), "missing")"#,
            r#"reachable(from-records((), ()), ("missing",))"#,
            r#"reachable(from-records((), ()), (), direction: "both")"#,
            r#"reachable(from-records((), ()), none)"#,
            r#"induced(from-records((), ()), ("missing",))"#,
            r#"induced(from-records((), ()), none)"#,
            r#"select-edges(from-records(("a",), ((source: "a", target: "a"),)), e => none)"#,
        ] {
            reject_package("graph", &format!("#let value = {expression}"));
        }
    }

    #[test]
    fn graph_endpoints_identify_unknown_keys() {
        for (expression, endpoint) in [
            (
                r#"from-records(("a",), ((source: "missing", target: "a"),))"#,
                "source",
            ),
            (
                r#"from-records(("a",), ((source: "a", target: "missing"),))"#,
                "target",
            ),
        ] {
            let (_directory, world) = package_world(&package_program(
                "graph",
                &format!("#let graph = {expression}"),
            ));
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            for detail in ["index 0", endpoint, "missing"] {
                assert!(error.message.contains(detail), "{error:?}");
            }
        }
    }

    #[test]
    fn package_guards_locate_calls() {
        for (package, expression, details) in [
            ("collection", "pick(none, ())", ["`fields`", "none"]),
            ("collection", "pick(auto, ())", ["`fields`", "auto"]),
            ("collection", "index-by((1,))", ["`key`", "`int`"]),
            (
                "collection",
                r#"index-by(("same", "same"))"#,
                ["unique keys", "same"],
            ),
            (
                "collection",
                r#"group-by-keys(("tag",))"#,
                ["`keys`", "`str`"],
            ),
            (
                "collection",
                r#"descendants(("a",), "root", key => if key == "a" { "b" } else { "a" })"#,
                ["cycle", "a"],
            ),
            ("schema", "min-length(str, -1)", ["`minimum`", "`-1`"]),
            ("schema", "schema(())", ["`fields`", "`array`"]),
            ("schema", "check(int, none)", ["function", "none"]),
            (
                "schema",
                "parse(1, check(int, _ => none))",
                ["check callback", "none"],
            ),
            (
                "schema",
                "parse(1, convert(int, _ => 7))",
                ["callback", "`7`"],
            ),
            (
                "schema",
                r#"issue("invalid", path: (-1,))"#,
                ["`path`", "index 0"],
            ),
            ("source", "parse-sources(none, int)", ["`sources`", "none"]),
            ("source", "parse-sources((7,), int)", ["index 0", "`7`"]),
        ] {
            let source = format!(
                "#import \"@tola/{package}:0.0.0\": *\n\
                 #let selected = {expression}\n\
                 #document(\"index.html\")[]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(error.location.path.as_deref(), Some("main.typ"));
            let range = error.location.range.unwrap();
            assert_eq!((range.start.line, range.end.line), (1, 1), "{error:?}");
            let line = source.lines().nth(1).unwrap();
            assert_eq!(
                &line[range.start.character..range.end.character],
                expression,
                "{error:?}"
            );
            assert!(matches!(
                error.origin,
                tola_typst::DiagnosticOrigin::Producer(_)
            ));
            for detail in details {
                assert!(error.message.contains(detail), "{expression}: {error:?}");
            }
            assert!(!error.message.contains("assertion failed"), "{error:?}");
            assert!(!error.message.contains("panicked"), "{error:?}");
        }
    }

    #[test]
    fn package_callbacks_keep_panic_spans() {
        for (package, expression) in [
            ("collection", r#"index-by(("a",), key: fail)"#),
            ("schema", "parse(1, map(int, fail))"),
        ] {
            let source = format!(
                "#import \"@tola/{package}:0.0.0\": *\n\
                 #let fail(value) = panic(\"callback unavailable\")\n\
                 #let selected = {expression}\n\
                 #document(\"index.html\")[]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(error.location.path.as_deref(), Some("main.typ"));
            let range = error.location.range.unwrap();
            assert_eq!((range.start.line, range.end.line), (1, 1), "{error:?}");
            let line = source.lines().nth(1).unwrap();
            assert_eq!(
                &line[range.start.character..range.end.character],
                r#"panic("callback unavailable")"#,
                "{error:?}"
            );
            assert_eq!(error.origin, tola_typst::DiagnosticOrigin::Typst);
            assert!(error.message.contains("callback unavailable"), "{error:?}");
            assert!(
                error.traces.iter().any(|trace| {
                    trace.location.path.as_deref() == Some("main.typ")
                        && trace.location.line == Some(3)
                }),
                "{error:?}"
            );
        }
    }

    mod collection {
        use super::{assert_package, reject_package};

        fn assert_collection(assertions: &str) {
            assert_package("collection", assertions);
        }

        fn reject_collection(assertions: &str) {
            reject_package("collection", assertions);
        }

        #[test]
        fn membership_shapes_are_not_coerced() {
            for expression in [
                "group-by(((tags: (\"rust\",)),), key: member => member.tags)",
                "group-by-keys(((tag: \"rust\"),), keys: member => member.tag)",
                "group-by-keys((none,))",
                "select-members(((tags: none),), (\"rust\",), keys: member => member.tags)",
                "select-members(((\"rust\",),), \"rust\")",
                "pick((name: \"Author\"), (1,))",
            ] {
                reject_collection(&format!("#let selected = {expression}"));
            }
        }

        #[test]
        fn sparse_parents_remain_traversable() {
            assert_collection(
                r#"
#let members = ("leaf", "root")
#let parent(key) = (leaf: "missing", missing: "root", root: none).at(key)
#assert.eq(ancestors(members, "leaf", parent), ("root",))
#assert.eq(descendants(members, "root", parent), ("leaf",))
#assert.eq(children(members, "missing", parent), ("leaf",))
#assert.eq(descendants(members, "missing", parent), ("leaf",))
"#,
            );
        }

        #[test]
        fn hierarchy_accepts_composite_identities() {
            assert_collection(
                r#"
#let root = (id: ("root", 1))
#let leaf = (id: ("leaf", 2))
#let parent(key) = if key == leaf.id { root.id } else { none }
#assert.eq(ancestors((leaf, root), leaf.id, parent, key: member => member.id), (root,))
#assert.eq(children((leaf, root), root.id, parent, key: member => member.id), (leaf,))
"#,
            );
        }

        #[test]
        fn duplicate_navigation_keys_are_errors() {
            for expression in [
                "index-by(members)",
                "adjacent(members, \"anchor\")",
                "window(members, \"anchor\", before: 1)",
                "children(members, \"root\", _ => none)",
                "descendants(members, \"root\", _ => none)",
                "ancestors(members, \"anchor\", _ => none)",
                "lineage(members, \"anchor\", _ => none)",
                "siblings(members, \"anchor\", _ => none)",
            ] {
                reject_collection(&format!(
                    "#let members = (\"anchor\", \"duplicate\", \"duplicate\")\n\
                     #let selected = {expression}"
                ));
            }
        }

        #[test]
        fn traversed_parent_cycles_are_errors() {
            for operation in ["ancestors", "lineage", "descendants"] {
                for parents in [
                    r#"(leaf: "leaf")"#,
                    r#"(leaf: "missing", missing: "leaf")"#,
                    r#"(leaf: "root", root: "leaf")"#,
                ] {
                    let anchor = if operation == "descendants" {
                        "root"
                    } else {
                        "leaf"
                    };
                    reject_collection(&format!(
                        "#let parent(key) = {parents}.at(key)\n\
                         #let selected = {operation}((\"leaf\",), \"{anchor}\", parent)"
                    ));
                }
            }
        }

        #[test]
        fn direct_relations_ignore_deeper_cycles() {
            assert_collection(
                r#"
#let members = ("leaf-a", "leaf-b", "root")
#let parent(key) = ("leaf-a": "root", "leaf-b": "root", root: "missing", missing: "root").at(key)
#assert.eq(children(members, "root", parent), ("leaf-a", "leaf-b"))
#assert.eq(siblings(members, "leaf-a", parent), ("leaf-b",))
"#,
            );
        }
    }

    /// The `@tola/assets` natives inside a real compilation.
    mod assets {
        use std::sync::Arc;

        use super::IconFiles;
        use tempfile::TempDir;

        use crate::package::SiteBindings;

        struct Site {
            directory: TempDir,
            config: crate::config::ResolvedSiteConfig,
            library: tola_packages::library::SiteLibrary,
        }

        /// One site with the given `assets` declarations on disk.
        ///
        /// `files` are `(source, url)` declarations; `tree` is an optional
        /// `(source directory, url prefix)` declaration. `settings` is the whole
        /// site configuration, so a caller asks for cache busting with
        /// `[assets] cache-busting = true` there.
        fn site(files: &[(&str, &str)], tree: Option<(&str, &str)>, settings: &str) -> Site {
            let directory = TempDir::new().unwrap();
            let mut config = crate::config::tests::load_test_config(directory.path(), settings);
            let mut declarations = Vec::new();
            for (name, url) in files {
                let source = directory.path().join(name);
                std::fs::create_dir_all(source.parent().unwrap()).unwrap();
                std::fs::write(&source, format!("/* {name} */")).unwrap();
                declarations.push(crate::config::section::AssetFileDeclaration::new(
                    &source,
                    crate::config::section::AssetUrl::parse(url).unwrap(),
                ));
            }
            if let Some((source, prefix)) = tree {
                let source = directory.path().join(source);
                std::fs::create_dir_all(&source).unwrap();
                std::fs::write(source.join("member.js"), "/* member */").unwrap();
                config.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
                    &source,
                    crate::config::section::AssetUrlPrefix::parse(prefix).unwrap(),
                )];
            }
            config.build.publish_dir = directory.path().join("public");
            config.assets.files = declarations;
            let library = library_for(&config);
            Site {
                directory,
                config,
                library,
            }
        }

        /// The inventory of one site's configured assets, rendered as a build renders it.
        fn configured_inventory(
            config: &crate::config::ResolvedSiteConfig,
        ) -> crate::asset::ConfiguredAssetInventory {
            crate::asset::render_configured_asset_inventory(
                config,
                &crate::resources::source_boundary(config, crate::InputScope::Online),
                &crate::cancellation::BuildCancellation::default(),
                None,
                &[],
            )
            .unwrap()
        }

        /// Bind one site's configured asset URLs the way a build does: from the
        /// rendered inventory, so the addresses are the ones this build publishes.
        fn library_for(
            config: &crate::config::ResolvedSiteConfig,
        ) -> tola_packages::library::SiteLibrary {
            let urls = configured_inventory(config).asset_urls(config).unwrap();
            let bindings = SiteBindings::from_config(config, urls);
            bindings.library(Default::default())
        }

        /// The identity of the published bytes of the one file at `name`.
        fn published_identity(config: &crate::config::ResolvedSiteConfig, name: &str) -> String {
            let inventory = configured_inventory(config);
            let (_, bytes) = inventory
                .entries()
                .find(|(output, _)| output.output.as_str() == name)
                .unwrap_or_else(|| panic!("{name} is published"));
            tola_typst::ContentDigest::of(bytes).to_hex()
        }

        /// Every output path this site's declarations published.
        fn published_names(config: &crate::config::ResolvedSiteConfig) -> Vec<String> {
            let mut names = configured_inventory(config)
                .entries()
                .map(|(output, _)| output.output.as_str().to_owned())
                .collect::<Vec<_>>();
            names.sort();
            names
        }

        impl Site {
            fn compile(&self, body: &str) -> Result<tola_typst::BundleCompilation, String> {
                let main = self.directory.path().join("site.typ");
                std::fs::write(
                    &main,
                    format!("#import \"@tola/address:0.0.0\": asset-url\n{body}"),
                )
                .unwrap();
                let files = tola_typst::FileResolver::new().with_provider(IconFiles::default());
                let world = tola_typst::TypstWorld::builder(&main, self.directory.path())
                    .with_files(Arc::new(files))
                    .with_shared_library(self.library.shared())
                    .with_local_cache()
                    .no_fonts()
                    .build(&tola_typst::BundleCancellation::default())
                    .expect("valid test world");
                tola_typst::compile_bundle_world(&world, &Default::default()).map_err(|error| {
                    error
                        .diagnostics()
                        .expect("Typst failure has diagnostics")
                        .errors()
                        .map(|diagnostic| {
                            // A diagnostic tells the author what failed and what to do about it,
                            // so the rendered failure has its hints too.
                            let hints = diagnostic
                                .hints
                                .iter()
                                .map(|hint| format!("hint: {}", hint.message))
                                .collect::<Vec<_>>()
                                .join("\n");
                            format!("{}\n{hints}", diagnostic.message)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }

            /// The value `asset-url(declared)` produced in a real Bundle compilation.
            fn resolve(&self, declared: &str) -> Result<String, String> {
                let compilation = self.compile(&format!(
                    "#document(\"index.html\")[#metadata(asset-url(\"{declared}\")) <resolved>]"
                ))?;
                let path = typst::syntax::VirtualPath::new("index.html").unwrap();
                let value = compilation
                    .document(&path)
                    .expect("the test document was compiled")
                    .metadata_unique("resolved")
                    .expect("one resolved URL")
                    .expect("the document declares the resolved URL");
                Ok(value
                    .cast::<typst::foundations::Str>()
                    .expect("asset-url returns a string")
                    .to_string())
            }
        }

        #[test]
        fn declared_urls_resolve_under_mount() {
            for (declared, expected) in [
                ("/app.js", "/blog/app.js"),
                ("/assets/index.html", "/blog/assets/index.html"),
                ("/a%23b.css", "/blog/a%23b.css"),
                ("/100%25.css", "/blog/100%25.css"),
                ("/café.css", "/blog/caf%C3%A9.css"),
            ] {
                let site = site(
                    &[("assets/app.js", declared)],
                    None,
                    "[site]\nbase-path = \"/blog/\"",
                );
                assert_eq!(site.resolve(declared).unwrap(), expected);
            }
        }

        #[test]
        fn cache_busting_appends_published_identity() {
            let site = site(
                &[("assets/app.js", "/app.js")],
                None,
                "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
            );
            let identity = published_identity(&site.config, "app.js");

            assert_eq!(
                site.resolve("/app.js").unwrap(),
                format!("/blog/app.js?h={identity}")
            );
            assert_eq!(published_names(&site.config), ["app.js"]);
        }

        #[test]
        fn cache_busting_follows_changed_bytes() {
            let mut site = site(
                &[("assets/app.js", "/app.js")],
                None,
                "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
            );
            std::fs::write(
                site.directory.path().join("assets/app.js"),
                "window.answer = 42;",
            )
            .unwrap();
            site.library = library_for(&site.config);
            let first = site.resolve("/app.js").unwrap();
            let first_identity = published_identity(&site.config, "app.js");
            assert_eq!(first, format!("/blog/app.js?h={first_identity}"));

            std::fs::write(
                site.directory.path().join("assets/app.js"),
                "window.answer = 43;",
            )
            .unwrap();
            site.library = library_for(&site.config);
            let changed_identity = published_identity(&site.config, "app.js");
            assert_ne!(changed_identity, first_identity);
            assert_eq!(
                site.resolve("/app.js").unwrap(),
                format!("/blog/app.js?h={changed_identity}")
            );
            assert_eq!(published_names(&site.config), ["app.js"]);
        }

        #[test]
        fn cache_busting_versions_tree_members() {
            let site = site(
                &[],
                Some(("assets", "/assets")),
                "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
            );
            let identity = published_identity(&site.config, "assets/member.js");

            assert_eq!(
                site.resolve("/assets/member.js").unwrap(),
                format!("/blog/assets/member.js?h={identity}")
            );
        }

        #[test]
        fn undeclared_urls_are_errors() {
            let site = site(
                &[("assets/app.js", "/app.js")],
                None,
                "[assets]\ncache-busting = true",
            );

            let error = site.resolve("/missing.js").unwrap_err();

            assert!(error.contains("/missing.js"), "{error}");
            assert!(error.contains("assets.files"), "{error}");
            assert!(error.contains("site configuration"), "{error}");
            assert!(!error.contains("tola.toml"), "{error}");
            assert!(
                !error.contains("/app.js"),
                "an undefined URL must not resolve to another declaration: {error}"
            );
        }

        #[test]
        fn unknown_tree_member_reports_error() {
            let site = site(&[], Some(("assets", "/assets")), "");

            assert_eq!(
                site.resolve("/assets/member.js").unwrap(),
                "/assets/member.js"
            );
            let error = site.resolve("/assets/missing.js").unwrap_err();
            assert!(error.contains("/assets/missing.js"), "{error}");
            assert!(error.contains("assets.files"), "{error}");
            assert_eq!(
                crate::filesystem::normalize_path(site.config.get_root()),
                crate::filesystem::normalize_path(site.directory.path()),
                "the site root is the temporary directory"
            );
        }

        #[test]
        fn exact_declaration_wins_over_tree() {
            let site = site(
                &[("assets/app.js", "/assets/app.js")],
                Some(("assets", "/assets")),
                "[assets]\ncache-busting = true\n[site]\nbase-path = \"/blog/\"",
            );

            assert_eq!(
                published_names(&site.config),
                ["assets/app.js", "assets/member.js"]
            );
            assert_eq!(
                site.resolve("/assets/app.js").unwrap(),
                format!(
                    "/blog/assets/app.js?h={}",
                    published_identity(&site.config, "assets/app.js")
                )
            );
            assert_eq!(
                site.resolve("/assets/member.js").unwrap(),
                format!(
                    "/blog/assets/member.js?h={}",
                    published_identity(&site.config, "assets/member.js")
                )
            );
        }
    }

    /// The `@tola/document` queries inside a real compilation.
    mod references {
        use super::*;
        use typst::foundations::Value;
        use typst::introspection::Location;

        struct QuerySite {
            directory: tempfile::TempDir,
            library: tola_packages::library::SiteLibrary,
            files: Arc<tola_typst::FileResolver>,
            cache: Arc<tola_typst::SharedFileCache>,
        }

        impl QuerySite {
            fn new(base_path: &str) -> Self {
                let directory = tempfile::TempDir::new().unwrap();
                std::fs::create_dir(directory.path().join("content")).unwrap();
                let library = Self::library_for_mount(directory.path(), base_path);
                Self {
                    directory,
                    library,
                    files: Arc::new(
                        tola_typst::FileResolver::new().with_provider(IconFiles::default()),
                    ),
                    cache: Arc::new(tola_typst::SharedFileCache::new()),
                }
            }

            fn library_for_mount(
                root: &Path,
                base_path: &str,
            ) -> tola_packages::library::SiteLibrary {
                let config = crate::config::tests::load_test_config(
                    root,
                    &format!(
                        "[site]\norigin = \"https://example.test\"\nbase-path = \"{base_path}\"\n"
                    ),
                );
                crate::package::SiteBindings::from_config(&config, Default::default())
                    .library(Default::default())
            }

            /// The world reading this site's program, with its own or the shared file cache.
            fn world(&self, source: &str, shared_cache: bool) -> tola_typst::TypstWorld {
                let main = self.directory.path().join("site.typ");
                std::fs::write(&main, source).unwrap();
                let world = tola_typst::TypstWorld::builder(&main, self.directory.path())
                    .with_files(Arc::clone(&self.files))
                    .with_shared_library(self.library.shared())
                    .no_fonts();
                if shared_cache {
                    world.with_shared_cache(Arc::clone(&self.cache))
                } else {
                    world.with_local_cache()
                }
                .build(&tola_typst::BundleCancellation::default())
                .unwrap()
            }

            fn compile(&self, source: &str, shared_cache: bool) -> tola_typst::BundleCompilation {
                tola_typst::compile_bundle_world(
                    &self.world(source, shared_cache),
                    &Default::default(),
                )
                .unwrap()
            }
        }

        fn records(
            compilation: &tola_typst::BundleCompilation,
            document: &str,
            label: &str,
        ) -> Vec<Dict> {
            let path = typst::syntax::VirtualPath::new(document).unwrap();
            compilation
                .document(&path)
                .unwrap()
                .metadata_unique(label)
                .unwrap()
                .unwrap()
                .cast::<Array>()
                .unwrap()
                .into_iter()
                .map(|value| value.cast::<Dict>().unwrap())
                .collect()
        }

        fn document(reference: &Dict) -> Dict {
            reference
                .get("document")
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap()
        }

        fn target(reference: &Dict) -> Dict {
            reference
                .get("target")
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap()
        }

        /// The site program listing every heading `body` writes through `headings()`.
        fn heading_listing(body: &str) -> String {
            format!(
                r#"#import "@tola/document:0.0.0": headings
#document("a/index.html")[
{body}  #context [#metadata(headings()) <headings>]
]
"#
            )
        }

        type LevelAndNesting = (u64, Option<u64>);

        /// The unresolved link carries the reason no target matched.
        #[test]
        fn every_link_gets_one_classification() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[
    #link("../b/#intro")[URL]
    #link(<intro>)[Label]
    #link("https://remote.test/elsewhere")[External]
    #link("https://example.test/b/")[Own origin]
    #link("/missing/")[Missing]
  ] <page>
  #context [#metadata(references(from: auto, from-within: <page>)) <outgoing>]
]
#document("b/index.html")[
  = Introduction <intro>
]
"#,
                true,
            );
            let outgoing = records(&compiled, "a/index.html", "outgoing");
            let resolutions = outgoing
                .iter()
                .map(|reference| reference.get("resolution").unwrap().clone())
                .collect::<Vec<_>>();
            assert_eq!(
                resolutions,
                ["found", "found", "external", "external", "unresolved"]
                    .map(|value| value.into_value())
            );
            let reason = outgoing[4]
                .get("reason")
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap();
            assert_eq!(reason.get("tag").unwrap(), &"no-such-target".into_value());
        }

        #[test]
        fn to_filter_selects_content() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[#html.main[see #link(<intro>)[intro] and #link("/b/")[page]] <page>]
#document("b/index.html")[
  #html.main[
    = Intro <intro>
    #context [
      #metadata(references(to: <intro>)) <section>
      #metadata(references(to: auto)) <document>
      #metadata(references(to: none)) <anywhere>
    ]
  ] <page>
]
"#,
                true,
            );
            assert_eq!(records(&compiled, "b/index.html", "section").len(), 1);
            assert_eq!(records(&compiled, "b/index.html", "document").len(), 2);
            assert_eq!(records(&compiled, "b/index.html", "anywhere").len(), 2);
        }

        /// `from-within` takes a label or a location, and restricts the query to that region.
        #[test]
        fn source_region_accepts_label_or_location() {
            let program = |scope: Option<&str>| {
                let argument =
                    scope.map_or_else(String::new, |scope| format!("from-within: {scope}"));
                format!(
                    r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("/b/")[Navigation]
  #html.main[#link("/b/")[Body]] <page>
  #context [#metadata(references({argument})) <region>]
]
#document("b/index.html")[B]
"#
                )
            };
            for scope in ["<page>", "query(selector(<page>)).first().location()"] {
                let site = QuerySite::new("/");
                let scoped = records(
                    &site.compile(&program(Some(scope)), true),
                    "a/index.html",
                    "region",
                );
                assert_eq!(
                    scoped.len(),
                    1,
                    "`from-within: {scope}` keeps only the region's reference"
                );

                let unscoped = records(
                    &site.compile(&program(None), true),
                    "a/index.html",
                    "region",
                );
                assert_eq!(
                    unscoped.len(),
                    2,
                    "without `from-within` the reference outside the region counts too"
                );
            }
        }

        #[test]
        fn location_scopes_the_reference_query() {
            let site = QuerySite::new("/");
            for (program, scope_reads_document) in [
                (
                    r#"
#import "@tola/document:0.0.0": headings, references
#document("b/index.html")[#html.main[#link(<intro>)[jump]] <page>]
#document("a/index.html")[
  = Intro <intro>
  #context [#metadata(references(to: headings().first().location)) <refs>]
]
"#,
                    false,
                ),
                (
                    r#"
#import "@tola/document:0.0.0": headings, references
#document("a/index.html")[
  = Intro <intro>
  #link("/b/")[to b]
  #context [#metadata(references(from: headings().first().location)) <refs>]
]
#document("b/index.html")[#link("/a/")[to a]]
"#,
                    true,
                ),
            ] {
                let compiled = site.compile(program, true);
                let found = records(&compiled, "a/index.html", "refs");
                assert_eq!(found.len(), 1, "{found:?}");
                let chosen = if scope_reads_document {
                    document(&found[0])
                } else {
                    target(&found[0])
                };
                assert_eq!(chosen.get("output").unwrap(), &"a/index.html".into_value());
            }
        }

        #[test]
        fn nesting_follows_outline_tree() {
            // Each shape: the headings it writes, and every heading's declared level with the
            // depth `outline()` draws it at.
            let shapes: [(&str, &[LevelAndNesting]); 4] = [
                (
                    "  = One\n  == Two\n  === Deep\n",
                    &[(1, Some(1)), (2, Some(2)), (3, Some(3))],
                ),
                (
                    "  = One\n  == Two\n  #heading(depth: 2, outlined: false)[Hidden]\n  === Deep\n",
                    &[(1, Some(1)), (2, Some(2)), (2, None), (3, Some(2))],
                ),
                (
                    "  = One\n  #heading(depth: 2, outlined: false)[Hidden]\n  === Deep\n",
                    &[(1, Some(1)), (2, None), (3, Some(2))],
                ),
                ("  = One\n  === Deep\n", &[(1, Some(1)), (3, Some(2))]),
            ];
            for (body, expected) in shapes {
                let site = QuerySite::new("/");
                let compiled = site.compile(&heading_listing(body), true);
                let listed: Vec<LevelAndNesting> = records(&compiled, "a/index.html", "headings")
                    .iter()
                    .map(|heading| {
                        (
                            heading.get("level").unwrap().clone().cast::<u64>().unwrap(),
                            heading
                                .get("nesting")
                                .unwrap()
                                .clone()
                                .cast::<Option<u64>>()
                                .unwrap(),
                        )
                    })
                    .collect();
                assert_eq!(listed, expected, "{body}");
            }
        }

        #[test]
        fn heading_body_preserves_source_content() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": headings
#document("index.html")[
  #show heading: it => [Replaced]
  = Introduction
  == *Bold* and _em_
  #context {
    assert.eq(
      headings().map(section => section.body),
      ([Introduction], [*Bold* and _em_]),
    )
  }
]
"#,
                true,
            );
        }

        /// A show rule decides which link reaches the output, and that link is what is reported.
        #[test]
        fn show_rule_picks_reported_link() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  #show link.where(dest: "/original/"): _ => link("/generated/")[Generated]
  #html.main[#link("/original/")[Original]] <page>
  #context [#metadata(references(from-within: <page>)) <outgoing>]
]
#document("original.html")[
  = Original <original>
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("generated/index.html")[
  Generated
  #context [#metadata(references(to: auto)) <incoming>]
]
"#,
                true,
            );
            let outgoing = records(&compiled, "from.html", "outgoing");
            assert_eq!(outgoing.len(), 1, "{outgoing:?}");
            assert_eq!(
                outgoing[0].get("destination").unwrap(),
                &"/generated/".into_value()
            );
            assert_eq!(
                target(&outgoing[0]).get("output").unwrap(),
                &"generated/index.html".into_value()
            );
            assert!(records(&compiled, "original.html", "incoming").is_empty());
            assert_eq!(
                records(&compiled, "generated/index.html", "incoming").len(),
                1
            );
        }

        /// A link written inside another link's body belongs to the outer one.
        /// A reference a show rule replaces is reported as what replaced it.
        /// Links a show rule produces are reported in the region that holds them.
        /// Occurrences stay in the region that holds them, wherever a derived query reads them.
        /// A chain of show rules reports the reference that finally reached the output.
        #[test]
        fn links_missing_from_the_output_stay_unreported() {
            let site = QuerySite::new("/");
            for rewritten in [
                "#show link: it => it.body",
                r#"#show link: it => html.a(href: "/generated/")[Raw]"#,
            ] {
                let compiled = site.compile(
                    &format!(
                        r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  {rewritten}
  #html.main[#link("/generated/")[Generated]] <page>
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("generated/index.html")[Generated]
"#
                    ),
                    true,
                );
                assert!(
                    records(&compiled, "from.html", "outgoing").is_empty(),
                    "{rewritten}"
                );
            }
        }

        #[test]
        fn escaped_urls_preserve_output_identity() {
            let cases = [
                (
                    r#"
#import "@tola/document:0.0.0": references
#document("100%-ready/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("index.html")[#link("/100%25-ready/")[Ready]]
"#,
                    "100%-ready/index.html",
                    "100%-ready/index.html",
                ),
                (
                    r#"
#import "@tola/document:0.0.0": references
#document("a#b/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("index.html")[#link("/a%23b/")[Hash]]
"#,
                    "a#b/index.html",
                    "a#b/index.html",
                ),
                (
                    // `%2541` escapes a literal `%41`, which must not decode to `aA`.
                    r#"
#import "@tola/document:0.0.0": references
#document("a%41", format: "html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("aA/index.html")[A]
#document("index.html")[#link("/a%2541")[Escaped percent]]
"#,
                    "a%41",
                    "a%41",
                ),
            ];
            for (program, document_path, output_name) in cases {
                let site = QuerySite::new("/");
                let compiled = site.compile(program, true);
                let incoming = records(&compiled, document_path, "incoming");
                assert_eq!(incoming.len(), 1, "{incoming:?}");
                assert_eq!(
                    incoming[0].get("resolution").unwrap(),
                    &"found".into_value()
                );
                assert_eq!(
                    target(&incoming[0]).get("output").unwrap(),
                    &output_name.into_value()
                );
            }
        }

        #[test]
        fn url_targets_carry_output_identity() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link(<target>)[Native target]
  #link("/target.html#a%2541")[URL target]
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("target.html")[
  #html.main[#html.div(id: "a%41")[Target] <target>] <body>
  #context [
    #metadata(references(to: <target>)) <by-label>
    #metadata(references(to: query(<target>).first().location())) <by-location>
    #metadata(references(to: <target-doc>)) <by-document>
    #metadata(references(to: <body>)) <body-endpoint>
    #metadata(references(to-within: <body>)) <body-targets>
    #metadata(references(to-within: <target>)) <target-descendants>
  ]
] <target-doc>
"#,
                true,
            );
            let native_location = records(&compiled, "target.html", "by-label")[0]
                .get("target")
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap()
                .get("location")
                .unwrap()
                .clone()
                .cast::<Location>()
                .unwrap();
            for filter in ["by-label", "by-location", "body-targets"] {
                let incoming = records(&compiled, "target.html", filter);
                assert_eq!(incoming.len(), 1, "{filter}: {incoming:?}");
                let target = target(&incoming[0]);
                assert_eq!(target.get("kind").unwrap(), &"element".into_value());
                assert_eq!(
                    target.get("location").unwrap(),
                    &native_location.into_value()
                );
                assert_eq!(target.get("fragment").unwrap(), &"a%41".into_value());
            }
            for filter in ["body-endpoint", "target-descendants"] {
                assert!(records(&compiled, "target.html", filter).is_empty());
            }
            for (output, label) in [("index.html", "outgoing"), ("target.html", "by-document")] {
                let incoming = records(&compiled, output, label);
                assert_eq!(incoming.len(), 2);
                let url = target(&incoming[1]);
                assert_eq!(url.get("kind").unwrap(), &"url".into_value());
                assert_eq!(url.get("location").unwrap(), &Value::None);
                assert_eq!(url.get("fragment").unwrap(), &"a%41".into_value());
            }
        }

        #[test]
        fn records_resolve_mounted_addresses() {
            let site = QuerySite::new("/docs/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link("/docs/guide/")[Clean]
  #link("/docs/guide/index.html?lang=en#part%20one")[Output alias]
  #link("/docs/page.html/")[Not a directory]
  #link("/docs/payload.json?version=2#entry")[Asset]
  #context [
    #metadata(references()) <outgoing>
  ]
]
#document("guide/index.html")[Guide]
#document("page.html")[Page]
#asset("payload.json", "{}")
"#,
                true,
            );
            let outgoing = records(&compiled, "index.html", "outgoing");
            assert_eq!(outgoing.len(), 4);
            assert_eq!(
                target(&outgoing[0]).get("output").unwrap(),
                &"guide/index.html".into_value()
            );
            assert_eq!(
                target(&outgoing[1]).get("route").unwrap(),
                &"/guide/".into_value()
            );
            assert_eq!(
                target(&outgoing[1]).get("fragment").unwrap(),
                &"part one".into_value()
            );
            assert_eq!(
                outgoing[2].get("resolution").unwrap(),
                &"unresolved".into_value()
            );
            assert_eq!(
                target(&outgoing[3]).get("output").unwrap(),
                &"payload.json".into_value()
            );
            assert_eq!(
                target(&outgoing[3]).get("query").unwrap(),
                &"version=2".into_value()
            );
        }

        #[test]
        fn origin_matched_url_stays_external() {
            let site = QuerySite::new("/docs/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("https://example.test/docs/b/")[Literal]
  #link("https://example.test/elsewhere/")[Outside the mount]
  #context [#metadata(references(from: auto)) <outgoing>]
]
#document("b/index.html")[
  #context [#metadata(references(to: auto)) <incoming>]
]
"#,
                true,
            );
            // No destination names this build's outputs, so neither reaches them.
            assert!(records(&compiled, "b/index.html", "incoming").is_empty());
            let written = records(&compiled, "a/index.html", "outgoing");
            assert_eq!(written.len(), 2, "{written:?}");
            for reference in &written {
                assert_eq!(
                    reference.get("resolution").unwrap(),
                    &"external".into_value()
                );
                assert_eq!(reference.get("target").unwrap(), &Value::None);
            }
        }

        #[test]
        fn reference_queries_converge_without_warning() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #show link.where(dest: "/linked/"): _ => link("/other/")[Rewritten]
  #html.main[#link("/linked/")[Linked]] <body>
  #context {
    let written = references(from: auto, from-within: <body>)
    assert.eq(written.len(), 1)
    assert.eq(written.first().target.output, "other/index.html")
  }
]
#document("other/index.html")[
  Other
  #context {
    let incoming = references(to: auto, from-within: <body>)
    assert.eq(incoming.len(), 1)
  }
]
"#,
                true,
            );
            let messages = compiled
                .diagnostics()
                .raw()
                .iter()
                .map(|diagnostic| diagnostic.source().message.to_string())
                .collect::<Vec<_>>();
            assert!(
                !messages
                    .iter()
                    .any(|message| message.contains("did not converge")),
                "{messages:?}"
            );
        }
    }

    mod schema {
        use super::{assert_package, reject_package};

        fn assert_schema(assertions: &str) {
            assert_package("schema", assertions);
        }

        fn reject_schema(assertions: &str) {
            reject_package("schema", assertions);
        }

        #[test]
        fn optional_distinguishes_absence() {
            assert_schema(
                r#"
#let field = schema((value: optional(any, default: "fallback")))
#assert.eq(parse((:), field), (value: "fallback"))
#for value in (none, auto, false, 0, "", (), (:)) {
  assert.eq(parse((value: value), field), (value: value))
}
#assert.eq(parse((:), schema((value: optional(any)))), (:))
#assert.eq(parse((:), schema((value: optional(any, default: none)))), (value: none))
#let missing = try-parse((:), schema((value: any)))
#assert.eq(missing.issues.map(problem => (problem.code, problem.path)), (("schema.missing", ("value",)),))

#let label = optional(str)
#let child = schema((label: label))
#assert.eq(parse((child: (:)), schema((child: child))), (child: (:)))
#assert.eq(parse((:), schema((child: optional(child)))), (:))
#assert.eq(try-parse((:), schema((child: child))).issues.map(problem => problem.path), (("child",),))

#let absent = optional(str)
#for wrapped in (
  map(absent, _ => panic("map received absence")),
  convert(absent, _ => panic("convert received absence")),
  refine(absent, _ => panic("refine received absence"), message: "not reached"),
  check(absent, _ => panic("check received absence")),
  trim(absent),
  non-empty(absent),
  describe(absent, "Optional title"),
) {
  assert.eq(parse((:), schema((title: wrapped))), (:))
}
"#,
            );
        }

        #[test]
        fn defaults_use_the_current_context() {
            assert_schema(
                r#"
#let page = schema((size: optional(map(str, _ => text.size), default: "local")))
#document("small.html")[
  #set text(size: 10pt)
  #context assert.eq(parse((:), page), (size: 10pt))
]
#document("large.html")[
  #set text(size: 20pt)
  #context assert.eq(parse((:), page), (size: 20pt))
]
"#,
            );
        }

        #[test]
        fn defaults_follow_the_inner_schema() {
            assert_schema(
                r#"
#let page = schema((title: optional(map(str, title => title + "!"), default: "Title")))
#assert.eq(parse((:), page), (title: "Title!"))
#assert.eq(parse((title: "Explicit"), page), (title: "Explicit!"))
"#,
            );
        }

        #[test]
        fn invalid_input_never_uses_default() {
            assert_schema(
                r#"
#let page = schema((title: optional(str, default: "Untitled")))
#for value in (none, auto, false, 0, (), (:)) {
  let failed = try-parse((title: value), page)
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.map(problem => problem.code), ("schema.type",))
  assert.eq(failed.issues.map(problem => problem.path), (("title",),))
}
"#,
            );
        }

        #[test]
        fn partial_parses_remaining_fields() {
            assert_schema(
                r#"
#let page = schema((title: str, tags: optional(array-of(str), default: ())))
#assert.eq(partial(page, tags: ("notes",))(title: "Notes"), (title: "Notes", tags: ("notes",)))
#assert.eq(partial(page)(title: "Notes"), parse((title: "Notes"), page))
"#,
            );
        }

        #[test]
        fn partial_refuses_duplicate_field() {
            reject_schema(
                r#"
#let page = schema((title: str, layout: str))
#let note = partial(page, title: "Notes")
#note(title: "Other", layout: "page")
"#,
            );
        }

        #[test]
        fn defaults_list_declared_fallbacks_only() {
            assert_schema(
                r#"
#let child = schema((label: optional(str)))
#let page = schema((title: str, tags: optional(array-of(str), default: ()), child: child))
#assert.eq(defaults(page), (tags: ()))
#assert.eq(defaults(child), (:))
"#,
            );
        }

        #[test]
        fn nullable_bypasses_the_inner_schema() {
            assert_schema(
                r#"
#let never = map(any, _ => panic("unexpected nullable callback"))
#assert.eq(parse(none, nullable(never)), none)
#assert.eq(parse((:), schema((name: nullable(optional(str))))), (:))
#assert.eq(try-parse((:), schema((name: nullable(str)))).issues.first().code, "schema.missing")
"#,
            );
        }

        #[test]
        fn non_empty_checks_normalized_values() {
            assert_schema(
                r#"
#for inner in (trim(str), map(str, _ => ""), map(str, _ => ())) {
  let failed = try-parse(" ", non-empty(inner))
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.first().code, "schema.non-empty")
}
#assert.eq(parse(" ", trim(non-empty(str))), "")
"#,
            );
        }

        #[test]
        fn non_empty_does_not_trim() {
            assert_schema(
                r#"
#assert.eq(parse(" \t ", non-empty(str)), " \t ")
#for value in ("", (), (:)) {
  assert.eq(try-parse(value, non-empty(any)).issues.first().code, "schema.non-empty")
}
"#,
            );
        }

        #[test]
        fn trim_rejects_non_strings() {
            assert_schema(
                r#"
#for value in (none, auto, false, 0, (), (:)) {
  let failed = try-parse(value, trim(any))
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.first().code, "schema.type")
}
"#,
            );
        }

        #[test]
        fn object_failures_follow_field_order() {
            assert_schema(
                r#"
#let failed = try-parse(
  (extra: true, second: 2, first: "bad"),
  schema((first: int, second: str)),
)
#assert.eq(failed.keys().sorted(), ("issues", "ok"))
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (
  ("schema.type", ("first",)),
  ("schema.type", ("second",)),
  ("schema.unknown-key", ("extra",)),
))
"#,
            );
        }

        #[test]
        fn keep_retains_unvalidated_fields() {
            assert_schema(
                r#"
#assert.eq(
  parse((name: " Name ", extra: none), schema((name: trim(str)), unknown: "keep")),
  (name: "Name", extra: none),
)
"#,
            );
        }

        #[test]
        fn containers_collect_each_child_failure() {
            assert_schema(
                r#"
#let declaration = schema((rows: array-of(dictionary-of(int))))
#let failed = try-parse((rows: ((z: false, a: "wrong"), (only: none))), declaration)
#assert.eq(failed.keys().sorted(), ("issues", "ok"))
#assert.eq(failed.issues.map(problem => problem.path), (
  ("rows", 0, "z"), ("rows", 0, "a"), ("rows", 1, "only"),
))
"#,
            );
        }

        #[test]
        fn map_keeps_result_shaped_values() {
            assert_schema(
                r#"
#let payload = (ok: false, issues: ("ordinary content",), value: 7)
#assert.eq(parse(1, map(int, _ => payload)), payload)
#assert.eq(parse(1, convert(int, _ => ok(payload))), payload)
"#,
            );
        }

        #[test]
        fn conversion_checks_its_output_once() {
            assert_schema(
                r#"
#let output = map(str, value => value + "!")
#assert.eq(parse(7, map(int, str, output: output)), "7!")
#assert.eq(parse(7, convert(int, value => ok(str(value)), output: output)), "7!")
#let problem = issue("Not convertible", code: "conversion", path: ("part",))
#assert.eq(try-parse(7, convert(int, _ => err(problem))), err(problem))
"#,
            );
        }

        #[test]
        fn callback_protocol_errors_propagate() {
            for declaration in [
                "refine(int, _ => none, message: \"Rejected\")",
                "check(int, _ => none)",
                "check(int, _ => (\"not an issue\",))",
                "convert(int, _ => 1)",
                "convert(int, _ => (ok: true, value: 1, issues: ()))",
                "convert(int, _ => (ok: false, issues: ()))",
                "convert(int, _ => (ok: false, value: 1, issues: (issue(\"No\"),)))",
            ] {
                reject_schema(&format!("#let parsed = try-parse(1, {declaration})"));
            }
        }

        #[test]
        fn union_preserves_programmer_errors() {
            reject_schema(
                r#"#let parsed = try-parse(1, union(map(int, _ => panic("branch failure")), any))"#,
            );
        }

        #[test]
        fn union_selects_the_first_success() {
            assert_schema(
                r#"
#assert.eq(parse("raw", union(map(str, _ => "first"), map(str, _ => "second"))), "first")
#let choice = union(optional(str), optional(int, default: 7))
#assert.eq(parse((:), schema((value: choice))), (:))
#assert.eq(parse((value: 3), schema((value: choice))), (value: 3))
"#,
            );
        }

        #[test]
        fn union_folds_absent_branches() {
            assert_schema(
                r#"
#let failed = try-parse((:), schema((value: union(str, int))))
#assert.eq(failed.issues.len(), 1)
#let problem = failed.issues.first()
#assert.eq((problem.code, problem.path), ("schema.missing", ("value",)))
#assert.eq((problem.notes, problem.children), ((), ()))
"#,
            );
        }

        #[test]
        fn descriptions_override_same_node() {
            assert_schema(
                r#"
#let described = describe(optional(describe(int, "integer value")), "publication year")
#assert.eq(inspect(described).description, "publication year")
#assert.eq(try-parse("bad", described), try-parse("bad", optional(int)))
#assert.eq(
  try-parse((:), schema((value: union(describe(str, "Name"))))),
  try-parse((:), schema((value: union(str)))),
)
"#,
            );
        }

        #[test]
        fn generated_values_report_their_origin() {
            assert_schema(
                r#"
#let cause-of(input, declaration) = try-parse(input, declaration).issues.first()
#let cases = (
  (
    cause: cause-of((:), schema((rows: optional(array-of(schema((count: int))), default: ((count: "bad"),))))),
    path: ("rows",),
    origin: ("default", (0, "count")),
    labeled: ("$.rows", "$[0].count"),
  ),
  (
    cause: cause-of((value: "raw"), schema((value: map(str, _ => (count: "bad"), output: schema((count: int)))))),
    path: ("value",),
    origin: ("output", ("count",)),
    labeled: ("$.value", "$.count"),
  ),
)
#for case in cases {
  let cause = case.cause
  assert.eq(cause.code, "schema.type")
  assert.eq(cause.path, case.path)
  assert.eq((cause.origin.kind, cause.origin.path), case.origin)
  assert.eq(cause.children, ())
  let rendered = format-issues((cause,))
  assert(rendered.contains(case.labeled.at(0)))
  assert(rendered.contains(case.labeled.at(1)))
  // The cause leads, so the generated value's own message is rendered once.
  assert.eq(rendered.split("expected `int`, found `str`").len(), 2)
}
"#,
            );
        }

        #[test]
        fn refine_reports_its_message() {
            assert_schema(
                r#"
#let positive = refine(int, value => value > 0, message: "must be positive")
#assert.eq(parse(1, positive), 1)
#assert.eq(try-parse(0, positive).issues.map(problem => (problem.code, problem.message)), (
  ("custom", "must be positive"),
))
"#,
            );
        }

        #[test]
        fn relations_use_successful_projection() {
            assert_schema(
                r#"
#let fields = schema((title: str, start: map(int, value => value + 1), end: int, note: optional(str)))
#let relation = check(fields, values => {
  assert.eq(values, (start: 2, end: 1))
  (issue("End precedes start", code: "range", path: ("end",)),)
}, on: ("start", "end", "note"))
#let failed = try-parse((title: 7, start: 1, end: 1), relation)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (
  ("schema.type", ("title",)), ("range", ("end",)),
))
"#,
            );
        }

        #[test]
        fn failed_dependencies_block_relations() {
            assert_schema(
                r#"
#let fields = schema((start: int, end: int))
#let relation = check(fields, _ => panic("failed dependency reached callback"), on: ("start", "end"))
#let failed = try-parse((start: "bad", end: 2), relation)
#assert.eq(failed.issues.map(problem => problem.path), (("start",),))
"#,
            );
        }

        #[test]
        fn explicit_none_fails_its_relation() {
            assert_schema(
                r#"
#let relation = check(nullable(schema((count: int))), _ => (), on: ("count",))
#let failed = try-parse(none, relation)
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.first().code, "schema.type")
#assert.eq(failed.issues.first().path, ())
"#,
            );
        }

        #[test]
        fn relation_boundaries_are_declared() {
            for declaration in [
                "check(schema((count: int)), _ => (), on: (\"missing\",))",
                "check(map(schema((count: int)), value => value), _ => (), on: (\"count\",))",
                "check(convert(schema((count: int)), ok), _ => (), on: (\"count\",))",
                "check(union(schema((count: int)), dictionary), _ => (), on: (\"count\",))",
                "check(lazy(() => schema((count: int))), _ => (), on: (\"count\",))",
            ] {
                reject_schema(&format!("#let declaration = {declaration}"));
            }
        }

        #[test]
        fn wrong_arity_still_reports_member_failures() {
            assert_schema(
                r#"
#let declaration = tuple(optional(str), int)
#for values in ((none,), (none, "wrong", true)) {
  let failed = try-parse(values, declaration)
  assert.eq(failed.ok, false)
  assert(failed.issues.any(problem => problem.code == "schema.arity" and problem.path == ()))
  assert(failed.issues.any(problem => problem.code == "schema.type" and problem.path == (0,)))
  if values.len() > 1 {
    assert(failed.issues.any(problem => problem.code == "schema.type" and problem.path == (1,)))
  }
}
"#,
            );
        }

        #[test]
        fn tuple_rest_applies_its_schema() {
            assert_schema(
                r#"
#let declaration = tuple(int, rest: trim(str))
#assert.eq(parse((1, " two ", " three "), declaration), (1, "two", "three"))
#let failed = try-parse((1, false, none), declaration)
#assert.eq(failed.issues.map(problem => problem.path), ((1,), (2,)))
#assert.eq(parse((), tuple()), ())
"#,
            );
        }

        #[test]
        fn lazy_recursion_retains_child_paths() {
            assert_schema(
                r#"
#let node-schema() = schema((value: int, children: optional(array-of(lazy(node-schema)), default: ())))
#assert.eq(parse((value: 1, children: ((value: 2),)), node-schema()), (
  value: 1, children: ((value: 2, children: ()),),
))
#let failed = try-parse((value: 1, children: ((value: 2, children: ((value: "bad"),)),)), node-schema())
#assert.eq(failed.issues.map(problem => problem.path), (("children", 0, "children", 0, "value"),))
"#,
            );
        }

        #[test]
        fn lazy_schema_waits_for_node() {
            assert_schema(
                r#"
#let deferred = lazy(() => panic("absent recursive node was evaluated"))
#assert.eq(parse((:), schema((node: optional(deferred)))), (:))
"#,
            );
        }

        #[test]
        fn variant_runs_tagged_branch() {
            assert_schema(
                r#"
#let declaration = variant("kind",
  schema((kind: literal("note"), body: str)),
  schema((kind: literal("image"), width: map(int, _ => panic("inactive branch")))),
)
#assert.eq(parse((kind: "note", body: "Hello"), declaration), (kind: "note", body: "Hello"))
#for value in ((:), (kind: "unknown")) {
  let failed = try-parse(value, declaration)
  assert.eq(failed.ok, false)
  assert.eq(failed.issues.map(problem => problem.path), (("kind",),))
}
"#,
            );
        }

        #[test]
        fn malformed_declarations_are_errors() {
            for declaration in [
                "err()",
                "union()",
                "enum-of(())",
                "schema((count: value => true))",
                "schema((count: int), unknown: \"discard\")",
                "variant(\"kind\", schema((kind: optional(literal(\"note\")))))",
                "variant(\"kind\", schema((kind: map(literal(\"note\"), value => value))))",
                "variant(\"kind\", schema((kind: literal(\"note\"))), schema((kind: literal(\"note\"))))",
                "try-parse(1, lazy(() => \"not a schema\"))",
            ] {
                reject_schema(&format!("#let declaration = {declaration}"));
            }
        }

        #[test]
        fn enum_membership_selects_declared_values() {
            assert_schema(
                r#"
#let kinds = enum-of(("draft", "published"))
#assert.eq(parse("draft", kinds), "draft")
#assert.eq(try-parse("archived", kinds).issues.map(problem => (problem.code, problem.path)), (
  ("schema.enum", ()),
))
#assert.eq(try-parse((:), schema((kind: kinds))).issues.first().code, "schema.missing")
"#,
            );
        }

        #[test]
        fn one_or_many_accepts_arrays() {
            assert_schema(
                r#"
#assert.eq(parse(((1,), (2,)), one-or-many(array)), ((1,), (2,)))
#let failed = try-parse((1, 2), one-or-many(array))
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.map(problem => problem.path), ((0,), (1,)))
"#,
            );
        }

        #[test]
        fn scalar_failure_reports_no_index() {
            assert_schema(
                r#"
#let failed = try-parse((tags: 1), schema((tags: one-or-many(str))))
#assert.eq(failed.issues.map(problem => problem.path), (("tags",),))
#assert.eq(parse("tag", one-or-many(str)), ("tag",))
"#,
            );
        }

        #[test]
        fn array_members_never_become_absent() {
            assert_schema(
                r#"
#let declaration = array-of(optional(str, default: "fallback"))
#let failed = try-parse(("present", none), declaration)
#assert.eq(failed.ok, false)
#assert.eq(failed.issues.map(problem => (problem.code, problem.path)), (("schema.type", (1,)),))
#assert.eq(parse((), declaration), ())
"#,
            );
        }

        #[test]
        fn length_bounds_count_graphemes() {
            assert_schema(
                r#"
#let one = max-length(min-length(str, 1), 1)
#for value in ("e\u{301}", "\u{1100}\u{1161}") {
  assert.eq(parse(value, one), value)
}
#assert.eq(try-parse("", one).issues.first().code, "schema.min-length")
#assert.eq(try-parse("e\u{301}x", one).issues.first().code, "schema.max-length")
"#,
            );
        }

        #[test]
        fn length_bounds_count_container_members() {
            assert_schema(
                r#"
#let pair = max-length(min-length(any, 2), 2)
#for value in ((1, 2), (first: none, second: false)) {
  assert.eq(parse(value, pair), value)
}
#assert.eq(try-parse((1,), pair).issues.first().code, "schema.min-length")
#assert.eq(try-parse((a: 1, b: 2, c: 3), pair).issues.first().code, "schema.max-length")
#assert.eq(try-parse(2, pair).issues.first().code, "schema.type")
"#,
            );
        }

        #[test]
        fn regex_rules_use_native_search() {
            assert_schema(
                r#"
#assert.eq(parse(" x ", matches(str, regex("x"))), " x ")
#assert.eq(try-parse(" x ", matches(str, regex("^x$"))).issues.first().code, "schema.matches")
#assert.eq(try-parse(7, matches(any, regex("7"))).issues.first().code, "schema.type")
"#,
            );
        }

        #[test]
        fn numeric_bounds_are_inclusive() {
            assert_schema(
                r#"
#let bounded = at-most(at-least(any, 1), 2.5)
#for value in (1, 1.0, 2, 2.5) { assert.eq(parse(value, bounded), value) }
#assert.eq(try-parse(0.9, bounded).issues.first().code, "schema.at-least")
#assert.eq(try-parse(2.6, bounded).issues.first().code, "schema.at-most")
#for value in (true, "2", 2pt, 20%, none) {
  assert.eq(try-parse(value, bounded).issues.first().code, "schema.type")
}
"#,
            );
        }

        #[test]
        fn invalid_rule_options_are_errors() {
            for declaration in [
                "min-length(str, -1)",
                "max-length(array, 1.5)",
                "matches(str, \"x\")",
                "at-least(int, \"1\")",
                "at-most(int, 1pt)",
            ] {
                reject_schema(&format!("#let declaration = {declaration}"));
            }
        }
    }

    mod source_schema {
        use super::*;
        use std::collections::BTreeMap;

        fn source_world(
            program: &str,
            sources: &[(&str, &str)],
        ) -> (TempDir, tola_typst::TypstWorld) {
            let directory = TempDir::new().unwrap();
            let config = crate::config::tests::load_test_config(directory.path(), "");
            let root = config.get_root();
            let content = root.join("content");
            let files =
                Arc::new(tola_typst::FileResolver::new().with_provider(IconFiles::default()));
            let mut units = Vec::new();
            let mut declarations = BTreeMap::new();
            for (id, body) in sources {
                let path = content.join(id);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, body).unwrap();
                declarations.insert(path.clone(), super::declared_metadata(&path, root));
                units.push(ContentUnit {
                    root: content.clone(),
                    id: ContentId::new((*id).into()),
                    source: path,
                    layout: ContentSourceLayout::from_entry_path(Path::new(id)),
                });
            }
            let records = SourceSet::without_metadata(&units, &config)
                .unwrap()
                .with_metadata(&declarations)
                .inputs()
                .to_source_records();
            let bindings = SiteBindings::from_config(&config, Default::default());
            let library = bindings.library(records);
            let main = root.join("site.typ");
            fs::write(&main, program).unwrap();
            let world = TypstWorld::builder(&main, root)
                .with_files(files)
                .with_local_cache()
                .no_fonts()
                .with_shared_library(library.shared())
                .build(&tola_typst::BundleCancellation::default())
                .expect("valid source world");
            (directory, world)
        }

        fn source_errors(
            program: &str,
            sources: &[(&str, &str)],
        ) -> Vec<tola_typst::diagnostic::Diagnostic> {
            let (_directory, world) = source_world(program, sources);
            tola_typst::compile_bundle_world(&world, &Default::default())
                .expect_err("invalid source metadata must fail compilation")
                .diagnostics()
                .expect("Typst failure has diagnostics")
                .errors()
                .cloned()
                .collect()
        }

        #[test]
        fn parsed_metadata_leaves_sources_unchanged() {
            let (_directory, world) = source_world(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, map, trim
#let original = all-sources()
#let selected = (original.at(1), original.at(0))
#let declaration = schema((title: map(trim(str), title => title + "!")), unknown: "keep")
#let parsed = parse-sources(selected, declaration)
#assert.eq(parsed.map(source => source.id), ("posts/note.typ", "about.typ"))
#assert.eq(parsed.map(source => source.meta), ((title: "Note!", draft: false), (title: "About!", draft: true)))
#for (before, after) in selected.zip(parsed) {
  for key in ("id", "file", "path", "filename", "route-segments") {
    assert.eq(after.at(key), before.at(key))
  }
}
#assert.eq(all-sources(), original)
#assert.eq(original.map(source => source.meta.title), (" About ", " Note "))
#document("index.html")[]
"#,
                &[
                    (
                        "about.typ",
                        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" About \", draft: true))",
                    ),
                    (
                        "posts/note.typ",
                        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" Note \", draft: false))",
                    ),
                ],
            );
            tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
        }

        #[test]
        fn source_schemas_preserve_selected_order() {
            let (_directory, world) = source_world(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, map, trim
#let original = all-sources()
#let selected = (original.at(1), original.at(0))
#let page-schema = schema((title: trim(str)))
#let post-schema = schema((title: map(trim(str), title => title + "!"), rank: int))
#let parsed = parse-sources(selected, source => {
  if source.route-segments.first() == "posts" { post-schema } else { page-schema }
})
#assert.eq(parsed.map(source => source.id), ("posts/note.typ", "about.typ"))
#assert.eq(parsed.map(source => source.meta), ((title: "Note!", rank: 3), (title: "About")))
#for (before, after) in selected.zip(parsed) {
  assert.eq(after + (meta: before.meta), before)
}
#assert.eq(all-sources(), original)
#document("index.html")[]
"#,
                &[
                    (
                        "about.typ",
                        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" About \"))",
                    ),
                    (
                        "posts/note.typ",
                        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \" Note \", rank: 3))",
                    ),
                ],
            );
            tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
        }

        #[test]
        fn mixed_issues_keep_source_locations() {
            let errors = source_errors(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let parsed = parse-sources(all-sources(), source => {
  if source.route-segments.first() == "posts" {
    schema((title: str, rank: int))
  } else {
    schema((title: str))
  }
})
#document("index.html")[]
"#,
                &[
                    (
                        "about.typ",
                        "\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: 1))",
                    ),
                    (
                        "posts/note.typ",
                        "\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: false, rank: \"bad\"))",
                    ),
                ],
            );
            assert_eq!(
                errors
                    .iter()
                    .map(|error| (error.location.path.as_deref(), error.location.line))
                    .collect::<Vec<_>>(),
                [
                    (Some("content/about.typ"), Some(3)),
                    (Some("content/posts/note.typ"), Some(4)),
                    (Some("content/posts/note.typ"), Some(4)),
                ]
            );
            assert!(errors[0].message.contains("$.title"));
            assert!(errors[1].message.contains("$.title"));
            assert!(errors[2].message.contains("$.rank"));
        }

        #[test]
        fn schema_failures_name_source_types() {
            for (declaration, expected, actual) in [
                ("none", "parse-sources schema", "none"),
                ("(title: str)", "parse-sources schema", "dictionary"),
                ("source => none", "schema function", "none"),
                ("source => () => none", "schema function", "function"),
            ] {
                let errors = source_errors(
                    &format!(
                        r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#let parsed = parse-sources(all-sources(), {declaration})
#document("index.html")[]"#
                    ),
                    &[("note.typ", "")],
                );
                assert_eq!(errors.len(), 1, "{errors:?}");
                let error = &errors[0];
                assert_eq!(error.location.path.as_deref(), Some("site.typ"));
                assert_eq!(error.location.line, Some(2));
                assert!(error.message.contains(expected), "{error:?}");
                assert!(error.message.contains("content/note.typ"), "{error:?}");
                assert!(error.message.contains(&format!("`{actual}`")), "{error:?}");
                assert!(
                    error
                        .message
                        .contains("expected a Typst type or schema declaration"),
                    "{error:?}"
                );
                if actual == "dictionary" {
                    assert!(
                        error
                            .hints
                            .iter()
                            .any(|hint| hint.message.contains("schema(...)")),
                        "{error:?}"
                    );
                }
            }
        }

        #[test]
        fn callback_failures_keep_authored_locations() {
            for declaration in ["source => fail()", "source => { try-parse(none, none) }"] {
                let errors = source_errors(
                    &format!(
                        r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": try-parse
#let fail() = panic("schema unavailable")
#let parsed = parse-sources(all-sources(), {declaration})
#document("index.html")[]"#
                    ),
                    &[("note.typ", "")],
                );
                let error = &errors[0];
                assert_eq!(error.location.path.as_deref(), Some("site.typ"));
                assert!(!error.message.contains("function for"), "{error:?}");
                assert!(
                    error
                        .hints
                        .iter()
                        .any(|hint| hint.message.contains("content/note.typ")),
                    "{error:?}"
                );
                if declaration.contains("fail()") {
                    assert_eq!(error.location.line, Some(3));
                    assert!(error.message.contains("schema unavailable"));
                    assert!(
                        error
                            .traces
                            .iter()
                            .any(|trace| trace.location.path.as_deref() == Some("site.typ")),
                        "{error:?}"
                    );
                }
            }
        }

        #[test]
        fn absent_metadata_uses_source_defaults() {
            let (_directory, world) = source_world(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, optional, nullable, try-parse
#let declaration = schema((title: optional(nullable(str), default: "Untitled")))
#assert.eq(parse-sources(all-sources(), declaration).map(source => source.meta), (
  (title: "Untitled"), (title: none),
))
#assert.eq(try-parse(none, declaration).ok, false)
#assert.eq(all-sources().map(source => source.meta), (none, (title: none)))
#document("index.html")[]
"#,
                &[
                    ("absent.typ", ""),
                    (
                        "explicit.typ",
                        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: none))",
                    ),
                ],
            );
            tola_typst::compile_bundle_world(&world, &Default::default()).unwrap();
        }

        #[test]
        fn batch_issues_split_by_source() {
            let errors = source_errors(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let parsed = parse-sources(all-sources(), schema((title: str, rank: int)))
#document("index.html")[]
"#,
                &[
                    (
                        "about.typ",
                        "\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: 1, rank: false))",
                    ),
                    (
                        "posts/note.typ",
                        "\n\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: false, rank: \"bad\"))",
                    ),
                    ("absent.typ", ""),
                ],
            );
            let locations = errors
                .iter()
                .map(|error| (error.location.path.as_deref(), error.location.line))
                .collect::<Vec<_>>();
            assert_eq!(
                locations,
                [
                    (Some("content/about.typ"), Some(4)),
                    (Some("content/about.typ"), Some(4)),
                    (Some("content/posts/note.typ"), Some(5)),
                    (Some("content/posts/note.typ"), Some(5)),
                    (Some("content/absent.typ"), Some(1)),
                    (Some("content/absent.typ"), Some(1)),
                ]
            );
            for error in &errors[4..] {
                assert_eq!(error.location.column, Some(1));
            }
        }

        #[test]
        fn metadata_failure_carries_its_code() {
            let (directory, world) = source_world(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": check, issue, nullable, optional, schema, timestamp
#let published = optional(
  check(nullable(timestamp), value => {
    if value == none {
      (issue("Published blog pages need a publication date; use draft: true while writing pages."),)
    } else {
      ()
    }
  }),
  default: none,
)
#let parsed = parse-sources(all-sources(), schema((published: published)))
#document("index.html")[]
"#,
                &[(
                    "note.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((:))",
                )],
            );
            let error: anyhow::Error =
                tola_typst::compile_bundle_world(&world, &Default::default())
                    .expect_err("the schema default must fail")
                    .into();
            let diagnostics =
                crate::compiler::diagnostic::error_diagnostics(&error, directory.path())
                    .expect("a compile failure has diagnostics");
            assert_eq!(diagnostics[0].code, crate::codes::source::METADATA);
            assert!(
                diagnostics[0].message.starts_with("`content/note.typ`: "),
                "{}",
                diagnostics[0].message
            );
            // The record keeps the evidence; presentation decides whether to show it.
            assert!(!diagnostics[0].trace.is_empty());
        }
    }
}
