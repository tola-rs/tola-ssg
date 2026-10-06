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
    use typst::foundations::{Array, Dict, IntoValue, Str};

    #[derive(Default)]
    struct IconFiles(Arc<tola_icons::IconCollections>);

    impl tola_typst::FileProvider for IconFiles {
        fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
            let typst::syntax::VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
            if tola_packages::is_icon_file(package, path) {
                return Some(
                    tola_packages::icon_file_bytes(&self.0, package, path)
                        .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
                        .unwrap_or(tola_typst::FileTarget::Missing),
                );
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

    /// A world reading the given inputs with no Tola library, for exercising raw `sys.inputs`.
    fn world_with_inputs(source: &str, inputs: Dict) -> (TempDir, TypstWorld) {
        let directory = TempDir::new().unwrap();
        let main = directory.path().join("main.typ");
        fs::write(&main, source).unwrap();
        let files = tola_typst::FileResolver::new().with_provider(IconFiles::default());
        let world = TypstWorld::builder(&main, directory.path())
            .with_files(Arc::new(files))
            .with_local_cache()
            .no_fonts()
            .with_inputs_dict(inputs)
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

    /// A world compiling against a default site configuration with the embedded fonts, for tests
    /// whose HTML export lays content out (math frames).
    fn world_with_fonts(source: &str, sources: SourceRecords) -> (TempDir, TypstWorld) {
        let fonts = Arc::new(FontStore::with_options(
            FontOptions::new()
                .with_system_fonts(false)
                .with_embedded_fonts(true),
        ));
        typst_world(
            source,
            &default_library(sources),
            Arc::default(),
            Some(fonts),
        )
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

    /// Asserts a default site world rejects `source`.
    fn assert_default_fails(source: &str) {
        let (_directory, world) = world_from_default_config(source, SourceRecords::default());
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect_err("invalid source must fail compiling");
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
        let header = tola_packages::CaptureHeader::new(
            world.main(),
            "metadata",
            tola_packages::CaptureMode::Collect,
        );
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
    fn example_fences_preserve_source() {
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
    fn site_fences_see_the_example_site() {
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

    #[test]
    fn example_site_renders_math_frames() {
        let (_directory, world) = world_from_example_site(
            r#"#import "@tola/web:0.0.0": math-svg
#document("math.html")[#math-svg($a^2$)]"#,
        );
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        let html = exported_html(&compilation, "/math.html");
        assert!(html.contains("<svg"), "{html}");
    }

    #[test]
    fn unknown_fence_info_names_the_line() {
        for (docs, line, info) in [
            ("```rust\n#assert(true)\n```", 1, "rust"),
            (
                "Intro\n\n```typst sites\n#assert(true)\n```",
                3,
                "typst sites",
            ),
            (
                "```typst site extra\n#assert(true)\n```",
                1,
                "typst site extra",
            ),
        ] {
            let error = documented_examples(docs).unwrap_err();
            assert!(
                error.contains(&format!("documentation line {line}")),
                "{error}"
            );
            assert!(error.contains(&format!("`{info}`")), "{error}");
        }
    }

    /// Every error message of a failed compilation.
    fn failure_messages(failure: &tola_typst::BundleCompileFailure) -> Vec<String> {
        failure
            .diagnostics()
            .expect("Typst failure has diagnostics")
            .errors()
            .map(|diagnostic| diagnostic.message.to_string())
            .collect()
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

    /// Compiles `assertions` against `@tola/<package>` in a world with no Tola library, which must
    /// hold.
    fn assert_package(package: &str, assertions: &str) {
        let (_directory, world) =
            world_with_inputs(&package_program(package, assertions), Dict::new());
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect("package assertions must hold");
    }

    /// Asserts compiling `assertions` against `@tola/<package>` fails with diagnostics.
    fn reject_package(package: &str, assertions: &str) {
        let (_directory, world) =
            world_with_inputs(&package_program(package, assertions), Dict::new());
        tola_typst::compile_bundle_world(&world, &Default::default())
            .expect_err("invalid assertions must fail compiling")
            .diagnostics()
            .expect("Typst failure has diagnostics");
    }

    fn assert_html_fragments_in_order(html: &str, fragments: &[&str]) {
        let mut remaining = html;
        for fragment in fragments {
            let offset = remaining
                .find(fragment)
                .unwrap_or_else(|| panic!("missing ordered HTML fragment `{fragment}` in {html}"));
            remaining = &remaining[offset + fragment.len()..];
        }
    }

    /// The bytes the Bundle exported at `path`.
    fn exported_bytes(compilation: &tola_typst::BundleCompilation, path: &str) -> Vec<u8> {
        compilation
            .export_entries(
                &tola_typst::BundleOptions::default(),
                &tola_typst::BundleCancellation::default(),
                None,
            )
            .expect("the test Bundle exports")
            .iter()
            .find(|entry| entry.path().get_with_slash() == path)
            .unwrap_or_else(|| panic!("missing Bundle output {path}"))
            .bytes()
            .to_vec()
    }

    /// The UTF-8 text the Bundle exported at `path`.
    fn exported_html(compilation: &tola_typst::BundleCompilation, path: &str) -> String {
        String::from_utf8(exported_bytes(compilation, path)).unwrap()
    }

    /// The UTF-8 text of the document `path` in `source`'s default-world compilation.
    fn html_document(source: &str, path: &str) -> String {
        let (_directory, compilation) = compiled_from_default_config(source);
        exported_html(&compilation, path)
    }

    fn read_virtual_package_file(
        compilation: &tola_typst::BundleCompilation,
        expected: TolaPackage,
        expected_path: &Path,
    ) -> bool {
        compilation.file_reads().iter().any(|read| {
            matches!(
                read.evidence().locator(),
                ReadLocator::ProvidedPackage { package, path }
                    if TolaPackage::from_spec(package) == Some(expected)
                        && path == expected_path
            )
        })
    }

    fn read_virtual_package(
        compilation: &tola_typst::BundleCompilation,
        expected: TolaPackage,
    ) -> bool {
        read_virtual_package_file(compilation, expected, Path::new("lib.typ"))
    }

    fn observed_capability_call(
        compilation: &tola_typst::BundleCompilation,
        expected: TolaPackage,
    ) -> bool {
        compilation
            .file_reads()
            .iter()
            .any(|read| expected.owns_capability_read(read.evidence().locator()))
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

    fn icon_library() -> (
        tola_packages::library::SiteLibrary,
        Arc<tola_icons::IconCollections>,
    ) {
        let mut collection = tola_icons::IconCollection::new();
        collection.insert_svg("mark", r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" class="source-icon" style="stroke-width:2"><defs><linearGradient id="paint"><stop offset="0" stop-color="currentColor"/><stop offset="1" stop-color="red"/></linearGradient></defs><path fill="url(#paint)" d="M0 0h24v24H0z"/></svg>"##).unwrap();
        collection.insert_svg("viewport", r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><circle cx="20" cy="20" r="2"/></svg>"#).unwrap();
        let mut collections = tola_icons::IconCollections::new();
        collections.mount("brand", collection).unwrap();
        let collections = Arc::new(collections);
        (default_library(SourceRecords::default()), collections)
    }

    #[test]
    fn icons_render_with_unique_gradient_ids() {
        let (library, collections) = icon_library();
        let (_directory, world) = world_with_icons(
            r#"
#import "@tola/icon:0.0.0": icon, icon-bytes
#assert.eq(type(icon-bytes("brand:mark")), bytes)
#asset("images/mark.svg", icon-bytes("brand:mark"))
#asset("images/viewport.svg", icon-bytes("brand:viewport"))
#document("index.html")[
  #for _ in range(2) {
    icon("brand:mark", label: "Brand & mark", attrs: (CLASS: "instance", STYLE: "color: blue", height: "2em"))
  }
]
#document("second.html")[#icon("brand:mark")]
#document("viewport.html")[#icon("brand:viewport")]
"#,
            &library,
            Arc::clone(&collections),
        );
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        assert_eq!(
            exported_bytes(&compilation, "/images/mark.svg"),
            collections.get("brand", "mark").unwrap().svg().as_bytes()
        );
        let html = exported_html(&compilation, "/index.html");
        assert_eq!(html.matches("<svg ").count(), 2);
        assert_eq!(html.matches("aria-label=\"Brand &amp; mark\"").count(), 2);
        assert_eq!(html.matches("role=\"img\"").count(), 2);
        assert!(html.contains("class=\"source-icon instance\""), "{html}");
        assert!(html.contains("stroke-width"), "{html}");
        assert!(html.contains("color: blue"), "{html}");
        let svg_start = html.find("<svg ").unwrap();
        let svg_end = svg_start + html[svg_start..].find("</svg>").unwrap() + "</svg>".len();
        let inline_svg = roxmltree::Document::parse(&html[svg_start..svg_end]).unwrap();
        let root = inline_svg.root_element();
        assert_eq!(
            root.children()
                .filter(|child| child.is_element())
                .map(|child| child.tag_name().name())
                .collect::<Vec<_>>(),
            ["defs", "path"]
        );
        assert!(html.contains("stop-color=\"currentColor\""), "{html}");
        assert!(html.contains("stop-color=\"red\""), "{html}");
        let mut ids = Vec::new();
        for (start, _) in html.match_indices("<svg ") {
            let end = start + html[start..].find("</svg>").unwrap() + "</svg>".len();
            let svg = roxmltree::Document::parse(&html[start..end]).unwrap();
            let gradient = svg
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "linearGradient")
                .unwrap();
            let id = gradient.attribute("id").unwrap();
            let path = svg
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "path")
                .unwrap();
            let paint = path.attribute("fill").unwrap();
            let target = paint
                .strip_prefix("url(")
                .unwrap()
                .strip_suffix(')')
                .unwrap()
                .trim()
                .trim_matches(['\'', '"']);
            assert_eq!(target, format!("#{id}"));
            ids.push(id.to_owned());
        }
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let second = exported_html(&compilation, "/second.html");
        assert!(second.contains("aria-hidden=\"true\""), "{second}");
        assert!(second.contains("focusable=\"false\""), "{second}");
        assert!(second.contains("<linearGradient "), "{second}");
        let viewport = exported_html(&compilation, "/viewport.html");
        let start = viewport.find("<svg ").unwrap();
        let end = start + viewport[start..].find("</svg>").unwrap() + "</svg>".len();
        let svg = roxmltree::Document::parse(&viewport[start..end]).unwrap();
        let root = svg.root_element();
        assert_eq!(root.attribute("viewBox"), Some("0 0 24 24"));
        assert_eq!(root.attribute("width"), Some("1em"));
        assert_eq!(root.attribute("height"), Some("1em"));
        let original = collections.get("brand", "viewport").unwrap().svg();
        assert_eq!(
            exported_bytes(&compilation, "/images/viewport.svg"),
            original.as_bytes()
        );
        assert!(
            roxmltree::Document::parse(original)
                .unwrap()
                .root_element()
                .attribute("viewBox")
                .is_none()
        );
    }

    #[test]
    fn invalid_icon_calls_fail_compilation() {
        let (library, collections) = icon_library();
        for call in [
            "icon(\"brand:missing\")",
            "icon(\"brand:mark\", attrs: (\"aria-hidden\": \"false\"))",
            "icon(\"brand:mark\", attrs: (\"ARIA-HIDDEN\": \"false\"))",
            "icon(\"brand:mark\", attrs: (\"Role\": \"presentation\"))",
            "icon(\"brand:mark\", label: \"  \")",
        ] {
            let source =
                format!("#import \"@tola/icon:0.0.0\": icon\n#document(\"index.html\")[#{call}]");
            let (_directory, world) = world_with_icons(&source, &library, Arc::clone(&collections));
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect_err("invalid icon call must fail compilation");
        }
    }

    #[test]
    fn icon_refuses_paged_document() {
        let (library, collections) = icon_library();
        let source = "#import \"@tola/icon:0.0.0\": icon\n\
                      #document(\"manual.pdf\", format: \"pdf\")[#icon(\"brand:mark\")]";
        let (_directory, world) = world_with_icons(source, &library, Arc::clone(&collections));
        let failure =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect_err("a paged document cannot render an inline icon");
        let messages = failure_messages(&failure);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("icon() needs an HTML document")),
            "{messages:?}"
        );
    }

    #[test]
    fn icon_url_is_linkable_in_every_document() {
        let (library, collections) = icon_library();
        for document in ["\"index.html\"", "\"manual.pdf\", format: \"pdf\""] {
            let source = format!(
                "#import \"@tola/icon:0.0.0\": icon-url\n\
                 #document({document})[#link(icon-url(\"brand:mark\"))[brand]]"
            );
            let (_directory, world) = world_with_icons(&source, &library, Arc::clone(&collections));
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect("icon-url answers a URL either document can link to");
        }
    }

    #[test]
    fn directory_route_keeps_decoded_segments() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/address:0.0.0": route
#assert.eq(route(()), "/")
#assert.eq(route(("Posts", "Deep")), "/Posts/Deep/")
#assert.eq(route(("Café", "Leaf")), "/Café/Leaf/")
#let route = route(("guide",))
#document(route.slice(1) + "index.html")[Guide]"#,
        );
        assert!(
            compilation
                .document(&VirtualPath::new("guide/index.html").unwrap())
                .is_some()
        );
    }

    #[test]
    fn output_path_maps_each_route() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/address:0.0.0": route-to-output
#assert.eq(route-to-output("/"), "index.html")
#assert.eq(route-to-output("/guide/"), "guide/index.html")
#assert.eq(route-to-output("/guide"), "guide")
#assert.eq(route-to-output("/manual.pdf"), "manual.pdf")
#assert.eq(route-to-output("/404.html"), "404.html")
#document(route-to-output("/about.html"))[About]"#,
        );
        assert!(
            compilation
                .document(&VirtualPath::new("about.html").unwrap())
                .is_some()
        );
    }

    #[test]
    fn output_route_inverse_maps_index() {
        assert_default_compiles(
            r#"#import "@tola/address:0.0.0": route-to-output, output-to-route
#assert.eq(output-to-route("guide/index.html"), "/guide/")
#assert.eq(output-to-route("404.html"), "/404.html")
#assert.eq(output-to-route("about"), "/about")
#assert.eq(route-to-output(output-to-route("guide/index.html")), "guide/index.html")
#document("index.html")[Routes]"#,
        );
    }

    #[test]
    fn output_url_renders_mount_and_origin() {
        assert_default_compiles(
            r#"#import "@tola/address:0.0.0": output-to-url
#assert.eq(output-to-url("guide/index.html"), "/guide/")
#assert.eq(output-to-url("Hello World/index.html", base-path: "/docs/"), "/docs/Hello%20World/")
#assert.eq(output-to-url("404.html", base-path: "/docs/"), "/docs/404.html")
#assert.eq(
  output-to-url("guide/index.html", base-path: "/docs/", origin: "https://example.test"),
  "https://example.test/docs/guide/",
)
#document("index.html")[URLs]"#,
        );
    }

    #[test]
    fn output_url_refuses_origin_paths() {
        for origin in ["https://example.test/docs/", "https://example.test/."] {
            let source = format!(
                "#import \"@tola/address:0.0.0\": output-to-url\n\
                 #let url = output-to-url(\"index.html\", origin: {origin:?})\n\
                 #document(\"index.html\")[Body]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure = tola_typst::compile_bundle_world(&world, &Default::default())
                .expect_err("deployment paths belong to base-path");
            let messages = failure_messages(&failure).join("\n");
            assert!(
                messages.contains("pass the deployment path with `base-path`"),
                "{messages}"
            );
        }
    }

    #[test]
    fn decode_path_decodes_once() {
        assert_default_compiles(
            r#"#import "@tola/address:0.0.0": decode-url-path
#assert.eq(decode-url-path("/caf%C3%A9/"), "/café/")
#assert.eq(decode-url-path("/Hello%20World/"), "/Hello World/")
#assert.eq(decode-url-path("/plain/"), "/plain/")
#document("index.html")[Decoded]"#,
        );
    }

    #[test]
    fn slugify_keeps_its_options() {
        assert_default_compiles(
            r#"#import "@tola/address:0.0.0": slugify
#assert.eq(slugify("Hello World"), "hello-world")
#assert.eq(slugify("Hello World", mode: "ascii", case: "upper", separator: "_", language: "ja"), "HELLO_WORLD")
#assert.eq(("Hello World",).map(slugify.with(language: "ja")), ("hello-world",))
#document("index.html")[Slugs]"#,
        );
    }

    #[test]
    fn address_conversion_reads_no_site_or_source() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/address:0.0.0": route, route-to-output, output-to-route, output-to-url, decode-url-path
#let converted = (
  route(("guide",)),
  route-to-output("/guide/"),
  output-to-route("guide/index.html"),
  output-to-url("guide/index.html", base-path: "/"),
  decode-url-path("/caf%C3%A9/"),
)
#document("index.html")[#converted.len()]"#,
        );
        assert!(!read_virtual_package(&compilation, TolaPackage::Site));
        assert!(!read_virtual_package(&compilation, TolaPackage::Source));
    }

    /// The theme file the rendering tests color their code with: keyword scopes are red and bold,
    /// string scopes green and decorated.
    const LIGHT_CODE_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>name</key><string>Light</string>
  <key>settings</key><array>
    <dict><key>settings</key><dict><key>foreground</key><string>#222222</string></dict></dict>
    <dict><key>scope</key><string>keyword</string><key>settings</key><dict><key>foreground</key><string>#aa0000</string><key>fontStyle</key><string>bold</string></dict></dict>
    <dict><key>scope</key><string>string</string><key>settings</key><dict><key>foreground</key><string>#008800</string><key>fontStyle</key><string>italic underline</string></dict></dict>
  </array>
</dict></plist>
"#;

    /// The dark appearance's theme file: every declaration differs from the light one.
    const DARK_CODE_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>name</key><string>Dark</string>
  <key>settings</key><array>
    <dict><key>settings</key><dict><key>foreground</key><string>#eeeeee</string></dict></dict>
    <dict><key>scope</key><string>keyword</string><key>settings</key><dict><key>foreground</key><string>#0000aa</string></dict></dict>
    <dict><key>scope</key><string>string</string><key>settings</key><dict><key>foreground</key><string>#00aa00</string></dict></dict>
  </array>
</dict></plist>
"#;

    #[test]
    fn rendered_code_extends_its_container_attributes() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/code:0.0.0": render-code, code-themes
#document("themed.html")[
  #set raw(theme: code-themes.github)
  #show raw: render-code.with(attrs: (CLASS: "my-code", id: "sample"))
  ```rust
  fn main() { let s = "hi"; }
  ```
]
#document("plain.html")[
  #show raw: render-code
  #raw("let x = 1;")
  #raw("let y = 2;", lang: "typc")
  ```rust
  fn main() {}
  ```
]"#,
        );

        let themed = exported_html(&compilation, "/themed.html");
        assert!(
            themed
                .contains(r#"<pre class="tola-code my-code" id="sample"><code data-lang="rust">"#),
            "{themed}"
        );

        let plain = exported_html(&compilation, "/plain.html");
        assert!(
            plain.contains(r#"<code class="tola-code">let x = 1;</code>"#),
            "{plain}"
        );
        assert!(
            plain.contains(r#"<code class="tola-code" data-lang="typc">"#),
            "{plain}"
        );
        assert!(
            plain.contains(r#"<pre class="tola-code"><code data-lang="rust">"#),
            "{plain}"
        );
    }

    #[test]
    fn rendered_code_has_resolved_styles() {
        let source = r#"#import "@tola/code:0.0.0": render-code
#document("index.html")[
  #set raw(theme: "/light.tmTheme")
  #show raw: render-code
  #raw("let x = \"hi\"", lang: "rust", block: true)
]"#;
        let (directory, world) = world_from_default_config(source, SourceRecords::default());
        fs::write(directory.path().join("light.tmTheme"), LIGHT_CODE_THEME).unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect("the themed document compiles");
        let html = exported_html(&compilation, "/index.html");

        assert!(html.contains("--tola-code-color:#aa0000"), "{html}");
        assert!(html.contains("--tola-code-weight:bold"), "{html}");
        assert!(html.contains("--tola-code-decoration:underline"), "{html}");
    }

    #[test]
    fn dark_theme_runs_have_dark_declarations() {
        let source = r#"#import "@tola/code:0.0.0": render-code
#document("index.html")[
  #set raw(theme: "/light.tmTheme")
  #show raw: render-code.with(dark-theme: path("/dark.tmTheme"))
  #raw("let x = 1", lang: "rust", block: true)
]"#;
        let (directory, world) = world_from_default_config(source, SourceRecords::default());
        fs::write(directory.path().join("light.tmTheme"), LIGHT_CODE_THEME).unwrap();
        fs::write(directory.path().join("dark.tmTheme"), DARK_CODE_THEME).unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect("the dual-appearance document compiles");
        let html = exported_html(&compilation, "/index.html");

        assert!(html.contains("--tola-code-dark-color:#0000aa"), "{html}");
        assert!(html.contains("--tola-code-dark-weight:none"), "{html}");
    }

    #[test]
    fn theme_file_reads_become_dependencies() {
        let source = r#"#import "@tola/code:0.0.0": render-code
#document("index.html")[
  #set raw(theme: "/light.tmTheme")
  #show raw: render-code
  #raw("let x = 1", lang: "rust", block: true)
]"#;
        let (directory, world) = world_from_default_config(source, SourceRecords::default());
        fs::write(directory.path().join("light.tmTheme"), LIGHT_CODE_THEME).unwrap();
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .expect("the themed document compiles");

        assert!(
            compilation.file_reads().iter().any(|read| matches!(
                read.evidence().locator(),
                tola_typst::ReadLocator::Root(path)
                    if path == std::path::Path::new("light.tmTheme")
            )),
            "the theme file a block reads is a compilation dependency"
        );
    }

    #[test]
    fn math_svg_wrappers_have_class_role_and_alt() {
        let (_directory, world) = world_with_fonts(
            r#"#import "@tola/web:0.0.0": math-svg
#document("math.html")[
  Unnamed: #math-svg($x^2$)
  Named: #math-svg($y^2$, alt: "y squared")
  Display: #math-svg($ z^2 $)
  Styled: #math-svg($a$, attrs: (class: "custom-math"))
]"#,
            SourceRecords::default(),
        );
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        let html = exported_html(&compilation, "/math.html");

        assert_eq!(html.matches(r#"role="math""#).count(), 4, "{html}");
        assert_eq!(
            html.matches(r#"class="tola-math-inline""#).count(),
            2,
            "{html}"
        );
        assert_eq!(
            html.matches(r#"class="tola-math-block""#).count(),
            1,
            "{html}"
        );
        assert_eq!(
            html.matches(r#"class="tola-math-inline custom-math""#)
                .count(),
            1,
            "{html}"
        );
        assert!(html.contains(r#"aria-label="y squared""#), "{html}");
        assert_eq!(html.matches("aria-label=").count(), 1, "{html}");
        assert!(html.contains("<svg"), "{html}");
    }

    #[test]
    fn math_svg_respects_equation_settings() {
        let (_directory, world) = world_with_fonts(
            r#"#import "@tola/web:0.0.0": math-svg
#document("math.html")[
  #set math.equation(block: true, alt: "Inherited")
  #math-svg(math.equation([x]), attrs: (id: "inherited"))
  #math-svg(math.equation(block: false, alt: "Explicit", [x]), attrs: (id: "explicit"))
  #math-svg($x$, alt: none, attrs: (id: "unnamed"))
]"#,
            SourceRecords::default(),
        );
        let cancellation = tola_typst::BundleCancellation::default();
        let compilation = tola_typst::compile_bundle_world(&world, &cancellation).unwrap();
        let path = typst::syntax::VirtualPath::new("math.html").unwrap();
        let document = compilation.document(&path).unwrap();
        for (id, tag, class, alt) in [
            ("inherited", "div", "tola-math-block", Some("Inherited")),
            ("explicit", "span", "tola-math-inline", Some("Explicit")),
            ("unnamed", "span", "tola-math-inline", None),
        ] {
            let fragment = document
                .html_fragment(
                    &tola_typst::HtmlFragmentSelection::Id(id.into()),
                    &Default::default(),
                    &cancellation,
                )
                .unwrap();
            let xml = roxmltree::Document::parse(&fragment.html).unwrap();
            let wrapper = xml.root_element();
            assert_eq!(wrapper.tag_name().name(), tag, "{id}");
            assert_eq!(wrapper.attribute("class"), Some(class), "{id}");
            assert_eq!(wrapper.attribute("aria-label"), alt, "{id}");
        }
    }

    /// The docs' CSS repaints the literal colors `typst-svg` writes; a release that changes them
    /// shows up here.
    #[test]
    fn framed_math_paints_with_literal_colors() {
        let (_directory, world) = world_with_fonts(
            r#"#import "@tola/web:0.0.0": math-svg
#document("math.html")[Inline #math-svg($x^2$).]"#,
            SourceRecords::default(),
        );
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        let html = exported_html(&compilation, "/math.html");

        assert!(html.contains("fill=\"#000000\""), "{html}");
    }

    #[test]
    fn math_wrappers_refuse_invalid_options() {
        for (call, expected) in [
            ("math-svg(42)", "expects a math.equation"),
            ("math-svg($x$, alt: 1)", "`alt` must be"),
            ("math-svg($ x $, attrs: ())", "`attrs` must be a dictionary"),
            (
                "math-svg($ x $, attrs: (class: 1))",
                "`attrs` values must be strings",
            ),
            (
                "math-svg($x$, attrs: (role: \"img\"))",
                "belongs to the math wrapper",
            ),
            (
                "math-svg($ x $, attrs: (aria-label: \"wrong\"))",
                "belongs to the math wrapper",
            ),
            (
                "math-svg($x$, attrs: (ROLE: \"img\"))",
                "belongs to the math wrapper",
            ),
            (
                "math-svg($ x $, attrs: (ARIA-LABEL: \"wrong\"))",
                "set the alternative with `alt`",
            ),
        ] {
            let source = format!(
                "#import \"@tola/web:0.0.0\": math-svg\n\
                 #document(\"index.html\")[#{call}]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure = tola_typst::compile_bundle_world(&world, &Default::default())
                .expect_err("invalid math options must report their contract");
            let messages = failure_messages(&failure).join("\n");
            assert!(messages.contains(expected), "{call}: {messages}");
        }
    }

    #[test]
    fn math_show_rules_keep_paged_fallback() {
        let (_directory, world) = world_with_fonts(
            r#"#import "@tola/web:0.0.0": math-svg
#let math-template(body) = {
  show math.equation: math-svg
  body
}
#document("math.html")[#show: math-template
  Inline $x^2$ and display $ y^2 $.
]
#document("math.pdf", format: "pdf")[#show: math-template
  Inline $x^2$ and display $ y^2 $.
]"#,
            SourceRecords::default(),
        );
        let compilation = tola_typst::compile_bundle_world(&world, &Default::default())
            .expect("one math template supports both document targets");
        let html = exported_html(&compilation, "/math.html");
        assert_eq!(html.matches(r#"role="math""#).count(), 2, "{html}");
        assert!(exported_bytes(&compilation, "/math.pdf").starts_with(b"%PDF-"));
    }

    #[test]
    fn raw_value_built_in_code_is_refused() {
        let (_directory, world) = world_from_default_config(
            r#"#import "@tola/code:0.0.0": render-code
#document("code.html")[
  #render-code(raw("SELECT 1;", lang: "sql", block: true))
]"#,
            SourceRecords::default(),
        );
        let cancellation = tola_typst::BundleCancellation::default();
        let failure = tola_typst::compile_bundle_world(&world, &cancellation)
            .expect_err("a raw value built in code has no lines");
        let messages = failure_messages(&failure);
        assert!(
            messages
                .iter()
                .any(|message| message
                    .contains("render-code needs a code block from the document")),
            "{messages:?}"
        );
    }

    #[test]
    fn rendered_code_refuses_the_language_attribute() {
        for attribute in ["data-lang", "DATA-LANG"] {
            let source = format!(
                "#import \"@tola/code:0.0.0\": render-code\n\
                 #document(\"index.html\")[\n\
                   #show raw: render-code.with(attrs: ({attribute:?}: \"wrong\"))\n\
                   `code`\n\
                 ]"
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure = tola_typst::compile_bundle_world(&world, &Default::default())
                .expect_err("the language attribute belongs to the renderer");
            let messages = failure_messages(&failure).join("\n");
            assert!(
                messages.contains("belongs to the rendered code"),
                "{messages}"
            );
        }
    }

    #[test]
    fn code_stylesheet_url_names_the_reserved_output() {
        assert_default_compiles(
            r#"#import "@tola/code:0.0.0": code-stylesheet-url
#assert.eq(code-stylesheet-url(), "/_tola/code-stylesheet.css")
#document("index.html")[Code]"#,
        );
    }

    #[test]
    fn explicit_head_text_overrides_site() {
        let html = html_document(
            r#"
#import "@tola/web:0.0.0": head-metadata
#document("index.html")[
  #html.html[
    #html.head(head-metadata(
      (title: "Site title", description: "Site description"),
      title: [Page *bold* & _emphasis_],
      description: [Description with #sym.alpha],
      viewport: "width=900",
    ).join())
    #html.body[]
  ]
]
"#,
            "/index.html",
        );
        assert_html_fragments_in_order(
            &html,
            &[
                r#"<meta name="viewport" content="width=900">"#,
                "<title>Page bold &amp; emphasis</title>",
                r#"<meta name="description" content="Description with α">"#,
            ],
        );
        assert!(!html.contains("Site title"), "{html}");
        assert!(!html.contains("Site description"), "{html}");
    }

    #[test]
    fn head_defaults_use_the_supplied_site() {
        let html = html_document(
            r#"
#import "@tola/web:0.0.0": head-metadata
#import "@tola/site:0.0.0": site
#let supplied-site = site + (title: "Site title", description: "Site description")
#document("index.html", title: [Independent native title])[
  #html.html[
    #html.head(head-metadata(supplied-site).join())
    #html.body[]
  ]
]
"#,
            "/index.html",
        );
        assert_html_fragments_in_order(
            &html,
            &[
                "<title>Site title</title>",
                r#"<meta name="description" content="Site description">"#,
            ],
        );
        assert!(!html.contains("Independent native title"), "{html}");
    }

    #[test]
    fn explicit_none_omits_head_entries() {
        let html = html_document(
            r#"
#import "@tola/web:0.0.0": head-metadata
#document("index.html")[
  #html.html[
    #html.head(head-metadata(
      (title: "Site title", description: "Site description"),
      title: none, description: none, viewport: none,
    ).join())
    #html.body[]
  ]
]
"#,
            "/index.html",
        );
        assert!(html.contains(r#"<meta charset="utf-8">"#), "{html}");
        for omitted in ["<title>", r#"name="description""#, r#"name="viewport""#] {
            assert!(!html.contains(omitted), "{html}");
        }
    }

    #[test]
    fn blank_site_metadata_is_omitted() {
        // The generated scaffold configures `site.title` and `site.description` as
        // empty strings, not `none`. Emitting them would publish `<title></title>`
        // and a valueless `content` attribute, which claim an absent title.
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/web:0.0.0": head-metadata
#document("blank.html")[
  #html.html[
    #html.head(head-metadata((title: "", description: "   ")).join())
    #html.body[Blank site]
  ]
]
#document("named.html")[
  #html.html[
    #html.head(head-metadata((title: "Named site", description: "")).join())
    #html.body[Named site]
  ]
]"#,
        );
        let blank = exported_html(&compilation, "/blank.html");
        assert!(!blank.contains("<title>"), "{blank}");
        assert!(!blank.contains(r#"name="description""#), "{blank}");
        let named = exported_html(&compilation, "/named.html");
        assert!(named.contains("<title>Named site</title>"), "{named}");
        assert!(!named.contains(r#"name="description""#), "{named}");
    }

    #[test]
    fn blank_social_text_is_omitted() {
        let html = html_document(
            r#"
#import "@tola/web:0.0.0": open-graph, twitter-card
#document("index.html")[
  #html.html[
    #html.head[
      #open-graph(
        title: " Social ", kind: "article", url: "https://example.test/",
        description: " ", site-name: "\t", locale: "",
        images: (
          (url: "https://example.test/a.png", alt: " First ", secure-url: "", media-type: " "),
          (url: "https://example.test/b.png", alt: "Second", secure-url: none),
          (url: "https://example.test/c.png", alt: "Third"),
        ),
      ).join()
      #twitter-card(card: "summary", title: " Social ", description: " ", handle: "", creator: "\t").join()
    ]
    #html.body[]
  ]
]
"#,
            "/index.html",
        );
        assert!(
            html.contains(r#"<meta property="og:title" content="Social">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<meta property="og:image:alt" content="First">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<meta name="twitter:title" content="Social">"#),
            "{html}"
        );
        for omitted in [
            "og:description",
            "og:site_name",
            "og:locale",
            "og:image:secure_url",
            "og:image:type",
            "twitter:description",
            "twitter:site",
            "twitter:creator",
        ] {
            assert!(!html.contains(omitted), "{html}");
        }
    }

    #[test]
    fn head_metadata_rejects_invalid_values() {
        for call in [
            r#"head-metadata("not a dictionary")"#,
            r#"head-metadata((:), title: 42)"#,
            r#"head-metadata((:), description: 42)"#,
            r#"head-metadata((:), viewport: 42)"#,
            r#"head-metadata((:), charset: 42)"#,
        ] {
            let source = format!(
                "#import \"@tola/web:0.0.0\": head-metadata\n\
                 #document(\"index.html\")[#html.html[#html.head({call}.join()) #html.body[Body]]]"
            );
            assert_default_fails(&source);
        }
    }

    #[test]
    fn seo_constructors_keep_head_order() {
        let (_directory, compilation) = compiled_from_default_config(
            r##"#import "@tola/web:0.0.0": head-metadata
#import "@tola/web:0.0.0": canonical, open-graph, twitter-card

#let graph = open-graph(
  title: "Social page",
  kind: "article",
  url: "https://example.com/social/",
  description: "Explicit description",
  site-name: "Example",
  locale: "en_US",
  images: (
    (
      url: "https://example.com/first.png",
      alt: "First image",
      secure-url: "https://cdn.example.com/first.png",
      media-type: "image/png",
      width: 1200,
      height: 630,
    ),
    (url: "https://example.com/second.png", alt: "Second image"),
  ),
)
#let twitter = twitter-card(
  card: "summary_large_image",
  title: "Social page",
  description: "Explicit description",
  handle: "@example",
  creator: "@author",
  image: (url: "https://example.com/twitter.png", alt: "Twitter image"),
)

#let page-head = (
  canonical("https://example.com/social/"),
  graph,
  html.meta(name: "theme-color", content: "#112233"),
  twitter,
)
#let emit(entries) = {
  if type(entries) == array {
    for entry in entries { emit(entry) }
  } else {
    entries
  }
}

#document("ordered.html", format: "html")[
  #html.html[
    #html.head[
      #for entry in head-metadata((:)) { entry }
      #emit(page-head)
    ]
    #html.body[Ordered constructor output]
  ]
]"##,
        );
        let html = exported_html(&compilation, "/ordered.html");
        assert_html_fragments_in_order(
            &html,
            &[
                "<head>",
                r#"<meta charset="utf-8">"#,
                r#"<link rel="canonical" href="https://example.com/social/">"#,
                r#"<meta property="og:title" content="Social page">"#,
                r#"<meta property="og:type" content="article">"#,
                r#"<meta property="og:url" content="https://example.com/social/">"#,
                r#"<meta property="og:description" content="Explicit description">"#,
                r#"<meta property="og:site_name" content="Example">"#,
                r#"<meta property="og:locale" content="en_US">"#,
                r#"<meta property="og:image" content="https://example.com/first.png">"#,
                r#"<meta property="og:image:secure_url" content="https://cdn.example.com/first.png">"#,
                r#"<meta property="og:image:type" content="image/png">"#,
                r#"<meta property="og:image:width" content="1200">"#,
                r#"<meta property="og:image:height" content="630">"#,
                r#"<meta property="og:image:alt" content="First image">"#,
                r#"<meta property="og:image" content="https://example.com/second.png">"#,
                r#"<meta property="og:image:alt" content="Second image">"#,
                r##"<meta name="theme-color" content="#112233">"##,
                r#"<meta name="twitter:card" content="summary_large_image">"#,
                r#"<meta name="twitter:title" content="Social page">"#,
                r#"<meta name="twitter:description" content="Explicit description">"#,
                r#"<meta name="twitter:site" content="@example">"#,
                r#"<meta name="twitter:creator" content="@author">"#,
                r#"<meta name="twitter:image" content="https://example.com/twitter.png">"#,
                r#"<meta name="twitter:image:alt" content="Twitter image">"#,
                "</head>",
                "<body>",
                "Ordered constructor output",
                "</body>",
            ],
        );
    }

    #[test]
    fn seo_constructors_reject_invalid_values() {
        for source in [
            r#"#import "@tola/web:0.0.0": canonical
#document("index.html")[#metadata(canonical("/relative/"))]"#,
            r#"#import "@tola/web:0.0.0": open-graph
#document("index.html")[#metadata(open-graph(
  kind: "article",
  url: "https://example.com/",
  images: ((url: "https://example.com/image.png", alt: "Image"),),
))]"#,
            r#"#import "@tola/web:0.0.0": open-graph
#document("index.html")[#metadata(open-graph(
  title: "Title",
  kind: "article",
  url: "https://example.com/",
  images: (),
))]"#,
            r#"#import "@tola/web:0.0.0": open-graph
#document("index.html")[#metadata(open-graph(
  title: "Title",
  kind: "article",
  url: "https://example.com/",
  images: ((url: "https://example.com/image.png",),),
))]"#,
            r#"#import "@tola/web:0.0.0": open-graph
#document("index.html")[#metadata(open-graph(
  title: "Title",
  kind: "article",
  url: "https://example.com/",
  images: ((url: "https://example.com/image.png", alt: "Image", caption: "No"),),
))]"#,
            r#"#import "@tola/web:0.0.0": twitter-card
#document("index.html")[#metadata(twitter-card(card: "player", title: "Title"))]"#,
            r#"#import "@tola/web:0.0.0": twitter-card
#document("index.html")[#metadata(twitter-card(
  card: "summary_large_image",
  title: "Title",
))]"#,
            r#"#import "@tola/web:0.0.0": twitter-card
#document("index.html")[#metadata(twitter-card(
  card: "summary",
  title: "Title",
  image: (url: "https://example.com/image.png",),
))]"#,
        ] {
            assert_default_fails(source);
        }
    }

    #[test]
    fn twitter_reports_independent_failures() {
        let (_directory, world) = world_from_default_config(
            r#"
#import "@tola/web:0.0.0": twitter-card
#let entries = twitter-card(card: "summary_large_image", title: 7)
#document("index.html")[]
"#,
            SourceRecords::default(),
        );
        let failure = tola_typst::compile_bundle_world(&world, &Default::default())
            .expect_err("invalid title cannot hide the missing large image");
        let messages = failure_messages(&failure).join("\n");
        assert!(messages.contains("$.title"), "{messages}");
        assert!(messages.contains("$.image"), "{messages}");
    }

    #[test]
    fn runtime_provider_serves_builtin_packages() {
        for package in tola_packages::resolvable_packages() {
            for (path, source) in package.files() {
                let runtime_source = read_package(&package.spec(), &format!("/{path}"))
                    .expect("runtime provider contains every package file");
                assert_eq!(source.as_bytes(), runtime_source);
            }
        }
    }

    #[test]
    fn manifests_match_package_specs() {
        for package in TolaPackage::all() {
            let manifest = package
                .files()
                .find_map(|(path, contents)| (path == "typst.toml").then_some(contents))
                .expect("package has a manifest");
            let manifest =
                toml::from_str::<toml::Value>(&manifest).expect("valid package manifest");
            let package_table = manifest
                .get("package")
                .and_then(toml::Value::as_table)
                .expect("manifest has a package table");
            let version = package.spec().version.to_string();

            assert_eq!(version, "0.0.0");

            assert_eq!(
                package_table.get("name").and_then(toml::Value::as_str),
                Some(package.name())
            );
            assert_eq!(
                package_table.get("version").and_then(toml::Value::as_str),
                Some(version.as_str())
            );
            assert_eq!(
                package_table
                    .get("entrypoint")
                    .and_then(toml::Value::as_str),
                Some("lib.typ")
            );
        }
    }

    #[test]
    fn all_sources_lists_every_source_in_order() {
        let sources = source_records(&["about.typ", "about/index.typ", "index.typ"]);
        let source = r#"
#import "@tola/address:0.0.0": route
#import "@tola/source:0.0.0": all-sources
#assert.eq(all-sources().map(source => route(source.at("route-segments"))), ("/about/", "/about/", "/"))
#let eager = all-sources().map(source => source.id).join(", ")
#document("index.html")[#eager / #context all-sources().last().id]
"#;
        let (_directory, world) = world_from_default_config(source, sources);
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        assert!(observed_capability_call(&compilation, TolaPackage::Source));

        let html = exported_html(&compilation, "/index.html");
        assert!(
            html.contains("about.typ, about/index.typ, index.typ / index.typ"),
            "{html}"
        );
    }

    #[test]
    fn current_resolves_per_document_output() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"
#import "@tola/document:0.0.0": current-document
#document("first.html")[#context current-document().output]
#document("second.html")[#context current-document().output]
"#,
        );
        let first = exported_html(&compilation, "/first.html");
        let second = exported_html(&compilation, "/second.html");
        assert!(
            first.contains("first") && !first.contains("second"),
            "{first}"
        );
        assert!(
            second.contains("second") && !second.contains("first"),
            "{second}"
        );
    }

    #[test]
    fn location_scopes_queries_to_document() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/document:0.0.0": current-document
#let inspect(expected) = context {
  let page = current-document()
  assert.eq(page.keys().sorted(), ("location", "output", "route"))
  let headings = query(selector(heading).within(page.location))
  assert.eq(headings.map(heading => heading.body), expected)
}
#document("guide/index.html", title: [Guide])[
  = First
  #inspect(([First], [Second]))
  = Second
]
#document("other.html")[
  = Other
  #inspect(([Other],))
]"#,
        );

        assert_eq!(compilation.documents().count(), 2);
    }

    #[test]
    fn route_only_cleans_index_file_name() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/document:0.0.0": current-document
#document("post/index.html")[#context current-document().route]
#document("myindex.html")[#context current-document().route]"#,
        );
        assert!(exported_html(&compilation, "/post/index.html").contains("/post/"));
        assert!(exported_html(&compilation, "/myindex.html").contains("/myindex.html"));
    }

    #[test]
    fn current_rejects_missing_document_context() {
        let (_directory, world) = world_from_default_config(
            r#"
#import "@tola/document:0.0.0": current-document
#context current-document().output
"#,
            SourceRecords::default(),
        );
        let failure =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap_err();
        let diagnostic = failure
            .diagnostics()
            .expect("Typst failure has diagnostics")
            .errors()
            .find(|diagnostic| diagnostic.message.contains("current-document()"))
            .expect("missing current context diagnostic");

        assert_eq!(diagnostic.location.path.as_deref(), Some("main.typ"));
        // The wording is the package's to choose; that the author gets a next step is the contract.
        assert!(
            !diagnostic.hints.is_empty(),
            "the message names no next step: {diagnostic:?}"
        );
    }

    #[test]
    fn importing_capabilities_reads_no_sources() {
        let (_directory, compilation) = compiled_from_default_config(
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/document:0.0.0": current-document
#document("index.html")[Static]"#,
        );
        assert!(read_virtual_package(&compilation, TolaPackage::Source));
        assert!(read_virtual_package(&compilation, TolaPackage::Document));
        assert!(!observed_capability_call(&compilation, TolaPackage::Source));
    }

    #[test]
    fn capabilities_reject_user_arguments() {
        for source in [
            r#"#import "@tola/source:0.0.0": all-sources
#document("index.html")[#all-sources("unexpected")]"#,
            r#"#import "@tola/source:0.0.0": all-sources
#document("index.html")[#all-sources(unexpected: "named")]"#,
            r#"#import "@tola/source:0.0.0": all-sources
#let unexpected = ("spread",)
#document("index.html")[#all-sources(..unexpected)]"#,
            r#"#import "@tola/document:0.0.0": current-document
#document("index.html", context [#current-document("unexpected")])"#,
            r#"#import "@tola/document:0.0.0": current-document
#document("index.html", context [#current-document(unexpected: "named")])"#,
            r#"#import "@tola/document:0.0.0": current-document
#let unexpected = ("spread",)
#document("index.html", context [#current-document(..unexpected)])"#,
        ] {
            let (_directory, world) = world_from_default_config(source, SourceRecords::default());
            let failure = tola_typst::compile_bundle_world(
                &world,
                &tola_typst::BundleCancellation::default(),
            )
            .unwrap_err();
            let messages = failure_messages(&failure);
            assert!(
                messages
                    .iter()
                    .any(|message| message.contains("unexpected argument")),
                "{messages:?}"
            );
        }
    }

    #[test]
    fn capability_evidence_survives_reuse() {
        let (_directory, world) = world_from_default_config(
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/document:0.0.0": current-document
#document("index.html", context [#all-sources().len() / #current-document().output])"#,
            source_records(&["index.typ"]),
        );

        for _ in 0..2 {
            let compilation = tola_typst::compile_bundle_world(
                &world,
                &tola_typst::BundleCancellation::default(),
            )
            .unwrap();
            assert!(read_virtual_package(&compilation, TolaPackage::Source));
            assert!(read_virtual_package(&compilation, TolaPackage::Document));
            assert!(observed_capability_call(&compilation, TolaPackage::Source));
        }
    }

    #[test]
    fn source_identity_is_lexical() {
        let sources = source_records(&["document.typ", "helper.typ"]);
        let (directory, world) = world_from_default_config(
            r#"
#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": current-source
#import "@tola/document:0.0.0": current-document
#import "content/helper.typ": helper-source
#assert.eq(helper-source().filename, "helper.typ")
#let document-source = all-sources().first()
#document("first/index.html")[
  #include document-source.file
]
#document("second.html")[
  #include document-source.file
]
#document("listing.html")[
  #context assert.eq(current-document().output, "listing.html")
]
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
        fs::write(
            directory.path().join("content/document.typ"),
            r#"
#import "@tola/address:0.0.0": route
#import "@tola/document:0.0.0": current-document
#import "@tola/source:0.0.0": current-source
#let source = current-source()
#assert.eq(source.path, "document.typ")
#assert.eq(route(source.at("route-segments")), "/document/")
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
        assert_eq!(compilation.documents().count(), 3);
    }

    #[test]
    fn native_api_survives_package_imports() {
        let sources = source_records(&["document.typ"]);
        let (directory, world) = world_from_default_config(
            r#"
#import "@tola/source:0.0.0"
#import "@tola/web:0.0.0"
#import "@tola/document:0.0.0" as doc
#let document-source = source.all-sources().first()
#document("first/index.html")[#include document-source.file]
#document("second.html")[#include document-source.file]
#document("native.html")[
  #html.html[
    #html.head(web.head-metadata((title: "Native title", description: none)).join())
    #html.body[#context doc.current-document().output]
  ]
]
"#,
            sources,
        );
        fs::create_dir(directory.path().join("content")).unwrap();
        fs::write(
            directory.path().join("content/document.typ"),
            r#"
#import "@tola/source:0.0.0": current-source
#import "@tola/document:0.0.0": current-document
#current-source().filename / #context current-document().output
"#,
        )
        .unwrap();

        let compilation =
            tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        let first = exported_html(&compilation, "/first/index.html");
        let second = exported_html(&compilation, "/second.html");
        assert!(first.contains("document.typ"), "{first}");
        assert!(second.contains("document.typ"), "{second}");
        assert!(first.contains("first/index.html"), "{first}");
        assert!(!first.contains("second.html"), "{first}");
        assert!(second.contains("second.html"), "{second}");
        assert!(!second.contains("first/index.html"), "{second}");

        let native = exported_html(&compilation, "/native.html");
        assert!(native.contains("Native title"), "{native}");
        assert!(native.contains("native.html"), "{native}");
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
        fn pick_preserves_literal_requested_keys() {
            assert_collection(
                r#"
#let fields = (name: "Author", "a.b": none, a: (b: 1), email: "a@example.test", url: "https://example.test")
#let selected = pick(fields, ("url", "a.b", "missing", "name", "url"))
#assert.eq(selected.keys(), ("url", "a.b", "name"))
#assert.eq(selected, (url: "https://example.test", "a.b": none, name: "Author"))
#assert.eq(fields, (name: "Author", "a.b": none, a: (b: 1), email: "a@example.test", url: "https://example.test"))
"#,
            );
        }

        #[test]
        fn groups_follow_first_key_appearance() {
            assert_collection(
                r#"
#let members = ((id: 1, tag: "b"), (id: 2, tag: "a"), (id: 3, tag: "b"))
#let groups = group-by(members, key: member => member.tag)
#assert.eq(groups.keys(), ("b", "a"))
#assert.eq(groups.at("b"), (members.at(0), members.at(2)))
#assert.eq(groups.at("a"), (members.at(1),))
#assert.eq(index-by(members, key: member => str(member.id)).at("2"), members.at(1))
"#,
            );
        }

        #[test]
        fn memberships_dedup_within_each_member() {
            assert_collection(
                r#"
#let tagged = (title: "Tagged", tags: ("rust", "typst", "rust"))
#let untagged = (title: "Untagged", tags: ())
#let members = (tagged, untagged, tagged, (title: "Web", tags: ("web", "typst")))
#let groups = group-by-keys(members, keys: member => member.tags)
#assert.eq(groups.keys(), ("rust", "typst", "web"))
#assert.eq(groups.at("rust"), (tagged, tagged))
#assert.eq(groups.at("typst"), (tagged, tagged, members.last()))
"#,
            );
        }

        #[test]
        fn membership_filters_keep_explicit_none() {
            assert_collection(
                r#"
#let members = ((tags: (none,)), (tags: ()), (tags: ("rust", none)))
#let tags(member) = member.tags
#assert.eq(select-members(members, (none,), keys: tags), (members.at(0), members.at(2)))
#assert.eq(select-members(members, ("rust", none), keys: tags, match: "all"), (members.at(2),))
#assert.eq(select-members(members, (), keys: tags), ())
#assert.eq(select-members(members, (), keys: tags, match: "all"), members)
"#,
            );
        }

        #[test]
        fn membership_selection_keeps_input_order() {
            assert_collection(
                r#"
#let members = ((tags: ("web",)), (tags: ("rust", "web")), (tags: ("other",)), (tags: ("rust",)))
#let tags(member) = member.tags
#assert.eq(select-members(members, ("rust", "web", "rust"), keys: tags), (members.at(0), members.at(1), members.at(3)))
#assert.eq(select-members(members, ("rust", "web"), keys: tags, match: "all"), (members.at(1),))
"#,
            );
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
        fn adjacent_omits_only_missing_neighbors() {
            assert_collection(
                r#"
#assert.eq(adjacent((none, "middle", "last"), "middle"), (before: none, after: "last"))
#assert.eq(adjacent((none, "middle", "last"), none), (after: "middle"))
#assert.eq(adjacent(("first", "last"), "last"), (before: "first"))
#assert.eq(adjacent(("only",), "only"), (:))
"#,
            );
        }

        #[test]
        fn missing_navigation_has_no_anchor() {
            assert_collection(
                r#"
#let parent(_) = panic("missing anchor must not traverse parents")
#assert.eq(adjacent(("root",), "missing"), none)
#assert.eq(window(("root",), "missing", before: 1, after: 1), none)
#assert.eq(ancestors(("root",), "missing", parent), none)
#assert.eq(siblings(("root",), "missing", parent), none)
#assert.eq(ancestors(("root",), "root", _ => none), ())
#assert.eq(siblings(("root",), "root", _ => none), ())
"#,
            );
        }

        #[test]
        fn window_excludes_its_anchor() {
            assert_collection(
                r#"
#let members = ((id: 3), (id: 1), (id: 4), (id: 2))
#let key(member) = member.id
#assert.eq(window(members, 1, key: key, before: 9, after: 1), (members.at(0), members.at(2)))
#assert.eq(window(members, 2, key: key, before: 1, after: 9), (members.at(2),))
#assert.eq(window(members, 3, key: key, before: 2), ())
"#,
            );
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
        fn lineage_includes_the_located_member() {
            assert_collection(
                r#"
#let members = ("leaf", "root")
#let parent(key) = (leaf: "missing", missing: "root", root: none).at(key)
#assert.eq(lineage(members, "leaf", parent), ("root", "leaf"))
#assert.eq(lineage(members, "root", parent), ("root",))
#assert.eq(lineage(members, "absent", parent), none)
"#,
            );
        }

        #[test]
        fn descendants_keep_member_order() {
            assert_collection(
                r#"
#let members = ("leaf-b", "root", "middle", "leaf-a")
#let parent(key) = ("leaf-b": "middle", root: none, middle: "root", "leaf-a": "middle").at(key)
#assert.eq(descendants(members, "root", parent), ("leaf-b", "middle", "leaf-a"))
#assert.eq(siblings(members, "leaf-a", parent), ("leaf-b",))
#assert.eq(ancestors(members, "leaf-a", parent), ("root", "middle"))
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
            let site = site(
                &[("assets/app.js", "/app.js")],
                None,
                "[site]\nbase-path = \"/blog/\"",
            );

            assert_eq!(site.resolve("/app.js").unwrap(), "/blog/app.js");
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
        use typst::foundations::{Content, Value};
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

            /// The error messages of a program this site rejects.
            fn compile_errors(&self, source: &str) -> Vec<String> {
                match tola_typst::compile_bundle_world(
                    &self.world(source, true),
                    &Default::default(),
                ) {
                    Ok(_) => panic!("expected the program to fail compiling"),
                    Err(failure) => failure
                        .diagnostics()
                        .map(|diagnostics| {
                            diagnostics
                                .errors()
                                .map(|error| error.message.clone())
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }

            /// Where a program this site rejects reports its errors: the author's own file.
            fn error_locations(&self, source: &str) -> Vec<(Option<String>, Option<usize>)> {
                tola_typst::compile_bundle_world(&self.world(source, true), &Default::default())
                    .expect_err("expected the program to fail compiling")
                    .diagnostics()
                    .expect("Typst failure has diagnostics")
                    .errors()
                    .map(|error| (error.location.path.clone(), error.location.line))
                    .collect()
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

        /// Every error of a rejected program is reported in the author's own file.
        fn assert_author_error(site: &QuerySite, source: &str) {
            let locations = site.error_locations(source);
            assert!(
                !locations.is_empty(),
                "the program reports at least one error"
            );
            for (path, line) in &locations {
                assert_eq!(path.as_deref(), Some("site.typ"), "{locations:?}");
                assert!(line.is_some(), "{locations:?}");
            }
        }

        type LevelAndNesting = (u64, Option<u64>);

        #[test]
        fn records_report_resolved_targets() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[
    #link("../b/?kind=note#intro")[URL]
    #link(<intro>)[Label]
    @intro
    #link("https://remote.test/elsewhere?q=1#remote")[External]
    #link("https://example.test/b/")[Own origin]
    #link("/missing/?x=1#lost")[Missing]
  ] <page>
  #link("/navigation/")[Generated navigation]
  #context [#metadata(references(from: auto, from-within: <page>)) <outgoing>]
] <page-a>
#document("b/index.html")[
  #set heading(numbering: "1.")
  = Introduction <intro>
  #context [#metadata(references(to: auto, from-within: <page>)) <incoming>]
]
"#,
                true,
            );
            let outgoing = records(&compiled, "a/index.html", "outgoing");
            assert_eq!(outgoing.len(), 6);
            assert_eq!(
                outgoing[0].get("destination").unwrap(),
                &"../b/?kind=note#intro".into_value()
            );
            assert_eq!(
                target(&outgoing[0]).get("output").unwrap(),
                &"b/index.html".into_value()
            );
            assert_eq!(
                target(&outgoing[0]).get("query").unwrap(),
                &"kind=note".into_value()
            );
            for reference in &outgoing[..3] {
                assert_eq!(reference.get("resolution").unwrap(), &"found".into_value());
                assert_eq!(
                    target(reference).get("fragment").unwrap(),
                    &"intro".into_value()
                );
            }
            assert_eq!(
                outgoing[3].get("resolution").unwrap(),
                &"external".into_value()
            );
            // An absolute URL is external even when it spells `site.origin`.
            assert_eq!(
                outgoing[4].get("resolution").unwrap(),
                &"external".into_value()
            );
            assert_eq!(
                outgoing[5].get("resolution").unwrap(),
                &"unresolved".into_value()
            );
            let reason = outgoing[5]
                .get("reason")
                .unwrap()
                .clone()
                .cast::<Dict>()
                .unwrap();
            assert_eq!(reason.get("tag").unwrap(), &"no-such-target".into_value());
            assert_eq!(
                target(&outgoing[5]).get("query").unwrap(),
                &"x=1".into_value()
            );
            assert_eq!(
                target(&outgoing[5]).get("fragment").unwrap(),
                &"lost".into_value()
            );
            assert_eq!(records(&compiled, "b/index.html", "incoming").len(), 3);
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

        #[test]
        fn from_filter_selects_labelled_document() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[#html.main[#link("/b/")[A to B]] <page>] <a-doc>
#document("c/index.html")[#html.main[#link("/b/")[C to B]] <page>] <c-doc>
#document("b/index.html")[
  #html.main[
    #context [
      #metadata(references(from: <a-doc>)) <written-in-a>
      #metadata(references(from: <c-doc>)) <written-in-c>
      #metadata(references(from: auto)) <written-in-b>
    ]
  ] <page>
]
"#,
                true,
            );
            let in_a = records(&compiled, "b/index.html", "written-in-a");
            assert_eq!(in_a.len(), 1);
            assert_eq!(
                document(&in_a[0]).get("output").unwrap(),
                &"a/index.html".into_value()
            );
            assert_eq!(records(&compiled, "b/index.html", "written-in-c").len(), 1);
            assert!(records(&compiled, "b/index.html", "written-in-b").is_empty());
        }

        #[test]
        fn source_selections_require_document_ownership() {
            let site = QuerySite::new("/");
            for selection in [
                "<download>",
                "query(<download>).first().location()",
                "selector(<download>)",
            ] {
                let program = format!(
                    r#"
#import "@tola/document:0.0.0": references
#document("index.html")[#context references(from: {selection})]
#asset("guide.pdf", "PDF bytes") <download>
"#,
                );
                assert_author_error(&site, &program);
            }
            assert_author_error(
                &site,
                r#"
#import "@tola/document:0.0.0": references
#metadata("outside") <outside>
#document("index.html")[#context references(from: <outside>)]
"#,
            );
        }

        #[test]
        fn asset_targets_remain_queryable() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link("/guide.pdf")[Download]
  #link(<download>)[Download by label]
  #context {
    let download = query(<download>).first().location()
    for endpoint in (<download>, download) {
      let incoming = references(to: endpoint)
      assert.eq(incoming.len(), 2)
      assert(incoming.all(reference => reference.target.output == "guide.pdf"))
      assert.eq(incoming.first().target.kind, "url")
      assert.eq(incoming.first().target.location, none)
      assert.eq(incoming.last().target.kind, "output")
      assert.eq(incoming.last().target.location, download)
    }
  }
]
#asset("guide.pdf", "PDF bytes") <download>
"#,
                true,
            );
        }

        #[test]
        fn source_scope_selects_occurrences() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #link("/b/")[Navigation]
  #html.main[#link("/b/")[Body]] <page>
  #context [
    #metadata(references(from-within: <page>)) <regioned>
    #metadata(references()) <everywhere>
  ]
]
#document("b/index.html")[B]
"#,
                true,
            );
            assert_eq!(records(&compiled, "a/index.html", "regioned").len(), 1);
            assert_eq!(records(&compiled, "a/index.html", "everywhere").len(), 2);
        }

        #[test]
        fn source_location_selects_occurrences() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[Inside: #link("/b/")[to b]] <page>
  Outside: #link("/b/")[to b]
  #context [#metadata(references(from-within: query(selector(<page>)).first().location())) <region>]
]
#document("b/index.html")[B]
"#,
                true,
            );
            assert_eq!(records(&compiled, "a/index.html", "region").len(), 1);
        }

        #[test]
        fn from_location_selects_containing_document() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": headings, references
#document("a/index.html")[
  = Intro <intro>
  #link("/b/")[to b]
  #context [#metadata(references(from: headings().first().location)) <in-a>]
]
#document("b/index.html")[#link("/a/")[to a]]
"#,
                true,
            );
            let in_a = records(&compiled, "a/index.html", "in-a");
            assert_eq!(in_a.len(), 1, "{in_a:?}");
            assert_eq!(
                document(&in_a[0]).get("output").unwrap(),
                &"a/index.html".into_value()
            );
        }

        #[test]
        fn heading_location_selects_the_heading() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": headings, references
#document("b/index.html")[#html.main[#link(<intro>)[jump]] <page>]
#document("a/index.html")[
  = Intro <intro>
  #context [#metadata(references(to: headings().first().location)) <refs>]
]
"#,
                true,
            );
            let refs = records(&compiled, "a/index.html", "refs");
            assert_eq!(refs.len(), 1, "{refs:?}");
            assert_eq!(
                target(&refs[0]).get("output").unwrap(),
                &"a/index.html".into_value()
            );
            assert_eq!(
                target(&refs[0]).get("fragment").unwrap(),
                &"intro".into_value()
            );
        }

        #[test]
        fn url_targets_keep_output_identity_only() {
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
        fn whole_targets_have_no_fragment() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("index.html")[
  #link(<home>)[Home]
  #link(<other>)[Other]
  #link(<download>)[Download]
  #context [#metadata(references(from: auto)) <outgoing>]
] <home>
#document("other.html")[
  #link(<home>)[Home]
  #link(<other>)[Other]
  #link(<download>)[Download]
  #context [#metadata(references(from: auto)) <outgoing>]
] <other>
#asset("guide.pdf", "PDF bytes") <download>
"#,
                true,
            );
            for output in ["index.html", "other.html"] {
                let outgoing = records(&compiled, output, "outgoing");
                assert_eq!(outgoing.len(), 3, "{output}: {outgoing:?}");
                for reference in outgoing {
                    let target = target(&reference);
                    assert_eq!(target.get("kind").unwrap(), &"output".into_value());
                    assert!(
                        target
                            .get("location")
                            .unwrap()
                            .clone()
                            .cast::<Option<Location>>()
                            .unwrap()
                            .is_some()
                    );
                    assert_eq!(target.get("fragment").unwrap(), &Value::None);
                }
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
        fn number_is_the_headings_own_numbering() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                &heading_listing(
                    r#"  #set heading(numbering: "1.")
  = One
  == Two
  #outline()
"#,
                ),
                true,
            );
            let numbers: Vec<Option<Str>> = records(&compiled, "a/index.html", "headings")
                .iter()
                .map(|heading| {
                    heading
                        .get("number")
                        .unwrap()
                        .clone()
                        .cast::<Option<Str>>()
                        .unwrap()
                })
                .collect();
            assert_eq!(
                numbers,
                [Some(Str::from("1.")), Some(Str::from("1.1.")), None]
            );
        }

        #[test]
        fn heading_depth_selects_declared_levels() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": headings
#document("index.html")[
  #context {
    assert.eq(headings().map(section => section.level), (1, 3, 2))
    assert.eq(headings(depth: 2).map(section => section.level), (1, 2))
    assert.eq(headings(depth: 1).map(section => section.text), ("Introduction",))
    for section in headings(depth: 2).filter(section => section.outlined) [
      #link(section.location, section.text)
    ]
  }
  = Introduction <intro>
  === Deep
  #heading(depth: 2, outlined: false)[Hidden]
]
#document("other.html")[= Other document]
#document("empty.html")[#context assert.eq(headings(), ())]
"#,
                true,
            );
        }

        #[test]
        fn heading_depth_requires_positive_integer() {
            let site = QuerySite::new("/");
            for depth in ["0", "-1", "1.5", "\"2\"", "auto", "false"] {
                let errors = site.compile_errors(&format!(
                    r#"
#import "@tola/document:0.0.0": headings
#document("index.html")[#context headings(depth: {depth})]
"#,
                ));
                assert_eq!(errors.len(), 1, "{depth}: {errors:?}");
                assert!(
                    errors[0].contains("`depth` must be `none` or a positive integer"),
                    "{depth}: {errors:?}"
                );
            }
        }

        #[test]
        fn heading_numbers_keep_function_values() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": headings
#document("index.html")[
  #set heading(numbering: (..numbers) => strong(str(numbers.pos().last())))
  = Introduction
  #context assert.eq(headings().first().number, strong("1"))
]
"#,
                true,
            );
        }

        #[test]
        fn absolute_url_stays_external_on_its_own_origin() {
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
        fn shared_region_label_keeps_queries_per_document() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[#link("/b/")[to b]] <page>
  #context [#metadata(references(to: auto, from-within: <page>)) <incoming>]
]
#document("b/index.html")[
  #html.main[#link("/a/")[to a]] <page>
  #context [#metadata(references(to: auto, from-within: <page>)) <incoming>]
]
"#,
                true,
            );
            let to_a = records(&compiled, "a/index.html", "incoming");
            assert_eq!(to_a.len(), 1, "{to_a:?}");
            assert_eq!(
                document(&to_a[0]).get("output").unwrap(),
                &"b/index.html".into_value()
            );
            let to_b = records(&compiled, "b/index.html", "incoming");
            assert_eq!(to_b.len(), 1, "{to_b:?}");
            assert_eq!(
                document(&to_b[0]).get("output").unwrap(),
                &"a/index.html".into_value()
            );
        }

        #[test]
        fn ref_reports_the_label_it_names() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  #show ref: reference => [
    #link("/to/?via=ref#one")[First]
    #link("/to/?via=ref#two")[Second]
  ]
  #html.main[@original] <page>
  #context [#metadata(references(from-within: <page>)) <outgoing>]
]
#document("original.html")[
  = Original <original>
  #context [#metadata(references(to: auto)) <incoming>]
]
#document("to/index.html")[To]
"#,
                true,
            );
            let outgoing = records(&compiled, "from.html", "outgoing");
            assert_eq!(outgoing.len(), 1, "{outgoing:?}");
            assert_eq!(
                target(&outgoing[0]).get("output").unwrap(),
                &"original.html".into_value(),
                "{outgoing:?}"
            );
            assert_eq!(
                outgoing[0].get("resolution").unwrap(),
                &"found".into_value(),
                "{outgoing:?}"
            );
            assert_eq!(records(&compiled, "original.html", "incoming").len(), 1);
        }

        #[test]
        fn nested_relations_belong_to_outermost_reference() {
            for body in [
                r#"
  #show ref.where(target: <original>): _ => ref(<generated>)
  #html.main[@original] <page>
"#,
                r#"
  #show link.where(dest: "/original/"): _ => [@generated #link("/generated/")[Generated]]
  #html.main[#link("/original/")[Original]] <page>
"#,
                r#"
  #html.main[#link("/original/")[Original @generated #link("/generated/")[Inner]]] <page>
"#,
            ] {
                let site = QuerySite::new("/");
                let compiled = site.compile(
                    &format!(
                        r#"
#import "@tola/document:0.0.0": references
#set heading(numbering: "1.")
#document("from.html")[
{body}
  #context [#metadata(references(from-within: <page>)) <outgoing>]
]
#document("original/index.html")[= Original <original>]
#document("generated/index.html")[
  = Generated <generated>
  #context [#metadata(references(to: auto, from-within: <page>)) <incoming>]
]
"#,
                    ),
                    true,
                );
                let outgoing = records(&compiled, "from.html", "outgoing");
                assert_eq!(outgoing.len(), 1, "{outgoing:?}");
                assert_eq!(
                    target(&outgoing[0]).get("output").unwrap(),
                    &"original/index.html".into_value(),
                );
                assert!(records(&compiled, "generated/index.html", "incoming").is_empty());
            }
        }

        #[test]
        fn source_scopes_keep_global_reference_owners() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("from.html")[
  #show ref: _ => [
    #html.main[#link("/generated/")[First] #link("/generated/")[Second]] <decorative>
  ]
  @original
  #context [
    #metadata(references(from-within: <decorative>)) <region>
    #metadata(references(from: auto)) <document>
  ]
]
#document("original.html")[= Original <original>]
#document("generated/index.html")[Generated]
"#,
                true,
            );
            assert!(records(&compiled, "from.html", "region").is_empty());
            let outgoing = records(&compiled, "from.html", "document");
            assert_eq!(outgoing.len(), 1);
            assert_eq!(
                target(&outgoing[0]).get("output").unwrap(),
                &"original.html".into_value()
            );
        }

        #[test]
        fn mixed_rendering_preserves_region_ownership() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": references
#set heading(numbering: "1.")
#document("from.html")[
  #show link.where(dest: "/original/"): _ => [@generated #link("/generated/")[Generated]]
  #html.main[#link("/original/")[Original]] <page>
  #context {
    let found = references(from-within: <page>)
    assert.eq(found.len(), 1)
    assert.eq(found.first().target.output, "original/index.html")
  }
]
#document("inner.html")[
  #show ref.where(target: <original>): _ => [
    #html.main[#link("/generated/")[First] #link("/generated/")[Second]] <decorative>
  ]
  @original
  #context assert.eq(references(from-within: <decorative>), ())
]
#document("original/index.html")[= Original <original>]
#document("generated/index.html")[
  = Generated <generated>
  #context {
    let generated = query(<generated>).first()
    assert.eq(references(to: generated.location(), from-within: <page>), ())
  }
]
#document("empty.html")[
  #html.main[Empty] <empty>
  #context assert.eq(references(from-within: query(<empty>).first().location()), ())
]
"#,
                true,
            );
        }

        #[test]
        fn reference_chains_keep_global_ownership() {
            for body in [
                r#"
  #show ref.where(target: <original>): _ => ref(<middle>)
  #show ref.where(target: <middle>): _ => ref(<generated>)
  #show ref.where(target: <generated>): _ => [
    #html.main[#link("/generated/")[First] #link("/generated/")[Second]] <decorative>
  ]
  #html.main[@original] <page>
"#,
                r#"
  #html.main[
    #link("/original/")[Original #link("/middle/")[Middle
      #html.main[#link("/generated/")[Generated]] <decorative>
    ]]
  ] <page>
"#,
            ] {
                let site = QuerySite::new("/");
                site.compile(
                    &format!(
                        r#"
#import "@tola/document:0.0.0": references
#set heading(numbering: "1.")
#document("from.html")[
{body}
  #context {{
    let found = references(from-within: <page>)
    assert.eq(found.len(), 1)
    assert.eq(found.first().target.output, "original/index.html")
    assert.eq(references(from-within: <decorative>), ())
  }}
]
#document("original/index.html")[= Original <original>]
#document("middle/index.html")[= Middle <middle>]
#document("generated/index.html")[= Generated <generated>]
"#,
                    ),
                    true,
                );
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
        fn shared_queries_follow_changed_selections() {
            let mut site = QuerySite::new("/");
            for (destination, inside_label, source_inside, output, mount, resolved) in [
                ("<one>", "one", true, "b/index.html", "/", true),
                ("<one>", "two", true, "b/index.html", "/", true),
                ("<two>", "two", true, "b/index.html", "/", true),
                ("<two>", "two", false, "b/index.html", "/", true),
                ("<two>", "two", true, "renamed/index.html", "/docs/", true),
                (
                    "\"/docs/b/?view=card#two\"",
                    "two",
                    true,
                    "b/index.html",
                    "/docs/",
                    true,
                ),
                (
                    "\"/docs/b/?view=card#two\"",
                    "two",
                    true,
                    "b/index.html",
                    "/",
                    false,
                ),
            ] {
                site.library = QuerySite::library_for_mount(site.directory.path(), mount);
                let link = format!("#link({destination})[Read]");
                let (inside_source, outside_source) = if source_inside {
                    (link.as_str(), "")
                } else {
                    ("", link.as_str())
                };
                let outside_label = if inside_label == "one" { "two" } else { "one" };
                let native = destination.starts_with('<');
                let target_location = if native {
                    format!(
                        "assert.eq(references(from: auto).first().target.location, query({destination}).first().location())"
                    )
                } else {
                    "assert.eq(references(from: auto).first().target.location, none)".to_owned()
                };
                let source = format!(
                    r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[{inside_source}] <page>
  {outside_source}
  #context [
    #metadata(references(from: auto)) <outgoing>
    #metadata(references(from: <source-doc>, from-within: <page>, to: <target-doc>, to-within: <target-region>)) <incoming>
    #{target_location}
  ]
] <source-doc>
#document("{output}")[
  #html.main[
    = Selected <{inside_label}>
  ] <target-region>
  #html.main[
    = Other <{outside_label}>
  ]
] <target-doc>
"#
                );
                let reused = site.compile(&source, true);
                let fresh = site.compile(&source, false);
                for query in ["outgoing", "incoming"] {
                    let reused_references = records(&reused, "a/index.html", query);
                    let fresh_references = records(&fresh, "a/index.html", query);
                    for (compilation, references) in
                        [(&reused, &reused_references), (&fresh, &fresh_references)]
                    {
                        for reference in references {
                            let document = document(reference);
                            let element = reference
                                .get("element")
                                .unwrap()
                                .clone()
                                .cast::<Content>()
                                .unwrap();
                            let document_output = document
                                .get("output")
                                .unwrap()
                                .clone()
                                .cast::<Str>()
                                .unwrap();
                            let document_location = document
                                .get("location")
                                .unwrap()
                                .clone()
                                .cast::<Location>()
                                .unwrap();
                            for location in [document_location, element.location().unwrap()] {
                                assert_eq!(
                                    compilation.introspector().path(location).map(|path| path
                                        .get_with_slash()
                                        .trim_start_matches('/')
                                        .to_owned()),
                                    Some(document_output.as_str().to_owned()),
                                );
                            }
                            let target = target(reference);
                            if let Some(location) = target
                                .get("location")
                                .unwrap()
                                .clone()
                                .cast::<Option<Location>>()
                                .unwrap()
                            {
                                let target_output =
                                    target.get("output").unwrap().clone().cast::<Str>().unwrap();
                                assert_eq!(
                                    compilation.introspector().path(location).map(|path| path
                                        .get_with_slash()
                                        .trim_start_matches('/')
                                        .to_owned()),
                                    Some(target_output.as_str().to_owned()),
                                );
                            }
                        }
                    }
                    let semantics = |references: &[Dict]| {
                        references
                            .iter()
                            .map(|reference| {
                                (
                                    ["output", "route"]
                                        .map(|name| document(reference).get(name).unwrap().clone()),
                                    ["kind", "output", "route", "query", "fragment"]
                                        .map(|name| target(reference).get(name).unwrap().clone()),
                                    ["destination", "resolution", "reason"]
                                        .map(|name| reference.get(name).unwrap().clone()),
                                )
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(semantics(&reused_references), semantics(&fresh_references));
                    let expected = if query == "outgoing" {
                        1
                    } else {
                        usize::from(source_inside && destination == format!("<{inside_label}>"))
                    };
                    assert_eq!(reused_references.len(), expected);
                }
                let outgoing = records(&reused, "a/index.html", "outgoing");
                let target = target(&outgoing[0]);
                assert_eq!(
                    target.get("kind").unwrap(),
                    &(if native { "element" } else { "url" }).into_value()
                );
                assert_eq!(
                    target.get("output").unwrap(),
                    &if resolved {
                        output.into_value()
                    } else {
                        Value::None
                    }
                );
            }
        }

        #[test]
        fn selectors_preserve_occurrence_order() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a.html")[
  #html.main[
    #link(<one>)[First]
    #html.div[#link(<two>)[Second]] <panel>
    #link(<one>)[Third]
  ] <body>
  #html.main[Empty] <empty>
  #context [
    #metadata(references(from: <body>)) <from-labels>
    #metadata(references(from: selector(<body>).or(<panel>))) <from-or>
    #metadata(references(from: selector(<body>).and(<panel>))) <from-and>
    #metadata(references(from-within: <body>)) <source-labels>
    #metadata(references(from-within: selector(<body>).or(<panel>))) <source-or>
    #metadata(references(from-within: selector(<body>).and(<panel>))) <source-and>
    #metadata(references(to: heading)) <to-function>
    #metadata(references(to: selector(<one>).or(<two>))) <to-or>
    #metadata(references(to: selector(heading).and(<one>))) <to-and>
    #metadata(references(to-within: <targets>)) <target-labels>
    #metadata(references(from: <absent>)) <empty-from>
    #metadata(references(from-within: <absent>)) <empty-source>
    #metadata(references(from-within: <empty>)) <empty-region>
    #metadata(references(to: heading.where(level: 6))) <empty-to>
    #metadata(references(to: <targets>)) <body-endpoints>
    #metadata(references(to-within: <absent>)) <empty-target>
  ]
]
#document("b.html")[#html.main[#link(<one>)[Fourth]] <body>]
#document("targets.html")[
  #html.main[
    = One <one>
  ] <targets>
  #html.main[
    = Two <two>
  ] <targets>
]
"#,
                true,
            );
            let order = |references: &[Dict]| {
                references
                    .iter()
                    .map(|reference| {
                        (
                            document(reference).get("output").unwrap().clone(),
                            target(reference).get("fragment").unwrap().clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let expected = [
                ("a.html", "one"),
                ("a.html", "two"),
                ("a.html", "one"),
                ("b.html", "one"),
            ]
            .map(|(output, fragment)| (output.into_value(), fragment.into_value()));
            for selection in [
                "from-labels",
                "from-or",
                "source-labels",
                "source-or",
                "to-function",
                "to-or",
                "target-labels",
            ] {
                assert_eq!(
                    order(&records(&compiled, "a.html", selection)),
                    expected,
                    "{selection}"
                );
            }
            assert_eq!(
                order(&records(&compiled, "a.html", "to-and")),
                [
                    expected[0].clone(),
                    expected[2].clone(),
                    expected[3].clone()
                ]
            );
            for selection in [
                "from-and",
                "source-and",
                "empty-from",
                "empty-source",
                "empty-region",
                "empty-to",
                "body-endpoints",
                "empty-target",
            ] {
                assert!(
                    records(&compiled, "a.html", selection).is_empty(),
                    "{selection}"
                );
            }
        }

        #[test]
        fn native_destinations_require_unique_labels() {
            let site = QuerySite::new("/");
            assert_author_error(
                &site,
                r#"
#document("index.html")[#link(<shared>)[Ambiguous target]]
#document("a.html")[= A <shared>]
#document("b.html")[= B <shared>]
"#,
            );
        }

        #[test]
        fn source_and_target_scopes_compose() {
            let site = QuerySite::new("/");
            site.compile(
                r#"
#import "@tola/document:0.0.0": references
#document("a/index.html")[
  #html.main[
    #link(<inside>)[Selected] <source-reference>
    #link(<outside>)[Outside target region]
    #link(<b-chapter>)[Region itself]
    #link(<b-doc>)[Document itself]
    #link(<c-target>)[Other document]
    #link("/b/#inside")[URL without native endpoint]
  ] <body>
  #link(<inside>)[Outside source region]
  #context {
    let found = references(
      from: <a-doc>, from-within: <body>,
      to: <b-doc>, to-within: selector(<b-chapter>).or(<c-chapter>),
    )
    assert.eq(found.len(), 1)
    assert.eq(found.first().target.kind, "element")
    assert.eq(found.first().target.location, query(<inside>).first().location())
    assert.eq(references(from-within: <source-reference>), ())
    assert.eq(references(to-within: <inside>), ())
  }
] <a-doc>
#document("b/index.html")[
  #html.main[#html.div(id: "inside")[Inside] <inside>] <b-chapter>
  #html.div(id: "outside")[Outside] <outside>
] <b-doc>
#document("c.html")[
  #html.main[#link(<inside>)[Other source]] <body>
  #html.main[#html.div(id: "other")[Other] <c-target>] <c-chapter>
]
"#,
                true,
            );
        }

        #[test]
        fn rendered_anchors_keep_distinct_native_targets() {
            let site = QuerySite::new("/");
            let compiled = site.compile(
                r#"
#import "@tola/document:0.0.0": references
#show heading: it => it.body
#document("index.html")[
  = Target #label("inner") <outer>
  #link(<outer>)[Outer]
  #link(<inner>)[Inner]
  #context [
    #metadata(references(to: <outer>)) <outer-links>
    #metadata(references(to: query(<inner>).first().location())) <inner-links>
  ]
]
"#,
                true,
            );
            let outer = records(&compiled, "index.html", "outer-links");
            let inner = records(&compiled, "index.html", "inner-links");
            assert_eq!(outer.len(), 1);
            assert_eq!(inner.len(), 1);
            let outer = target(&outer[0])
                .get("location")
                .unwrap()
                .clone()
                .cast::<Location>()
                .unwrap();
            let inner = target(&inner[0])
                .get("location")
                .unwrap()
                .clone()
                .cast::<Location>()
                .unwrap();
            assert_ne!(outer, inner);
            let anchor = compiled.introspector().anchor(outer).unwrap();
            assert_eq!(compiled.introspector().anchor(inner), Some(anchor));
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
"#,
            );
        }

        #[test]
        fn nested_fields_keep_optionality() {
            assert_schema(
                r#"
#let label = optional(str)
#let child = schema((label: label))
#assert.eq(parse((child: (:)), schema((child: child))), (child: (:)))
#assert.eq(parse((:), schema((child: optional(child)))), (:))
#let missing = try-parse((:), schema((child: child)))
#assert.eq(missing.issues.map(problem => problem.path), (("child",),))
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
        fn wrappers_preserve_field_omission() {
            assert_schema(
                r#"
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
        fn results_have_exclusive_payloads() {
            assert_schema(
                r#"
#assert.eq(ok(none), (ok: true, value: none))
#assert.eq(try-parse(none, any), (ok: true, value: none))
#let problem = issue("Rejected", code: "example", path: ("name", 0))
#assert.eq(problem, (code: "example", path: ("name", 0), message: "Rejected", notes: (), children: ()))
#assert.eq(err(problem), (ok: false, issues: (problem,)))
#let failed = try-parse(false, str)
#assert.eq(failed.keys().sorted(), ("issues", "ok"))
#assert.eq(failed.issues.first().code, "schema.type")
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
        fn inspection_preserves_output_metadata() {
            assert_schema(
                r#"
#let primitive = inspect(str)
#assert.eq(primitive, (kind: "type", presence: "required", description: none, type: str))
#let absent = inspect(optional(optional(str, default: "inner")))
#assert.eq(absent.presence, "optional")
#assert("default" not in absent)
#let nullable-default = inspect(nullable(describe(optional(str, default: none), "name")))
#assert.eq((nullable-default.presence, nullable-default.description, nullable-default.default), ("required", "name", (value: none)))
#assert.eq(nullable-default.members.map(member => (member.presence, member.description, "default" in member)), (("required", none, false), ("required", none, false)))
#let converted = inspect(map(
  describe(optional(str, default: "7"), "raw integer"),
  int,
  output: describe(optional(int, default: 9), "normalized integer"),
))
#assert.eq((converted.kind, converted.type, converted.presence, converted.default, converted.description), ("type", int, "required", (value: "7"), "normalized integer"))
#let omitted = inspect(convert(optional(str), value => ok(value), output: optional(str, default: "output")))
#assert.eq(omitted.presence, "optional")
#assert("default" not in omitted)
#let trimmed = inspect(trim(describe(optional(str, default: " padded "), "name")))
#assert.eq((trimmed.kind, trimmed.type, trimmed.default, trimmed.description), ("type", str, (value: " padded "), "name"))
#assert.eq(inspect(union(str, int)).presence, "required")
#assert.eq(inspect(union(str, optional(int))).presence, "optional")
#assert.eq(inspect(union(str, lazy(() => int))).presence, "unknown")
#assert("default" not in inspect(union(optional(str, default: "x"), optional(int))))
"#,
            );
        }

        #[test]
        fn inspection_describes_nested_shapes() {
            assert_schema(
                r#"
#let member = describe(optional(str, default: "item"), "member text")
#let page = inspect(schema(("a.b": optional(int), items: array-of(member)), unknown: "keep"))
#assert.eq((page.kind, page.unknown), ("object", "keep"))
#assert.eq(page.fields.at("a.b").presence, "optional")
#assert.eq(page.fields.items.element.description, "member text")
#for container in (array-of(member), one-or-many(member), dictionary-of(member)) {
  let element = inspect(container).element
  assert.eq(element.presence, "required")
  assert("default" not in element)
}
#let positions = inspect(tuple(member, rest: member))
#assert.eq(positions.kind, "tuple")
#assert.eq((positions.members.first().presence, positions.rest.presence), ("required", "required"))
#assert("default" not in positions.members.first() and "default" not in positions.rest)
#assert.eq(inspect(tuple(str)).rest, none)
#let alternatives = inspect(variant("kind",
  describe(schema((kind: literal("text"), body: describe(str, "body text"))), "text branch"),
  schema((kind: literal("count"), count: int)),
))
#assert.eq((alternatives.kind, alternatives.tag-key, alternatives.tags), ("variant", "kind", ("text", "count")))
#assert.eq(alternatives.branches.first().description, "text branch")
#assert.eq(alternatives.branches.first().fields.body.description, "body text")
#assert.eq(inspect(literal(auto)).value, auto)
#assert.eq(inspect(enum-of(("text", "count"))).values, ("text", "count"))
"#,
            );
        }

        #[test]
        fn inspection_never_evaluates_callbacks() {
            assert_schema(
                r#"
#let never(..arguments) = panic("inspection invoked user code")
#for declaration in (
  check(str, never),
  refine(str, never, message: "not reached"),
  map(str, never, output: int),
  convert(str, never, output: int),
  lazy(never),
  optional(int, default: "not an integer"),
  optional(any, default: never),
) {
  let description = inspect(declaration)
  assert("kind" in description)
}
#let deferred = inspect(describe(lazy(never), "recursive value"))
#assert.eq((deferred.kind, deferred.presence, deferred.description), ("unknown", "unknown", "recursive value"))
#assert.eq(inspect(optional(int, default: "invalid")).default, (value: "invalid"))
"#,
            );
        }

        #[test]
        fn union_keeps_relative_branch_paths() {
            assert_schema(
                r#"
#let choice = union(schema((a: int), unknown: "keep"), schema((b: str), unknown: "keep"))
#let failed = try-parse((rows: ((a: "bad", b: 2),)), schema((rows: array-of(choice))))
#let cause = failed.issues.first()
#assert.eq(cause.code, "schema.union")
#assert.eq(cause.path, ("rows", 0))
#assert.eq(cause.children.map(branch => (branch.code, branch.path)), (
  ("schema.branch", ()), ("schema.branch", ()),
))
#assert.eq(cause.children.map(branch => branch.children.map(problem => problem.path)), (
  (("a",),), (("b",),),
))
"#,
            );
        }

        #[test]
        fn later_checks_retain_default_origin() {
            assert_schema(
                r#"
#let field = non-empty(optional(trim(str), default: "  "))
#let cause = try-parse((:), schema((name: field))).issues.first()
#assert.eq((cause.code, cause.path), ("schema.default", ("name",)))
#assert.eq(cause.children.map(problem => (problem.code, problem.path)), (("schema.non-empty", ()),))
"#,
            );
        }

        #[test]
        fn formatted_paths_stay_unambiguous() {
            assert_schema(
                r#"
#let rendered = format-issues((
  issue("Nested", path: ("a", "b")),
  issue("Literal", path: ("a.b",)),
  issue("Indexed", path: (0,)),
  issue("Key", path: ("0",)),
))
#assert.eq(rendered.split("\n").map(line => line.slice(0, line.position(": "))), (
  "`$.a.b`", "`$[\"a.b\"]`", "`$[0]`", "`$[\"0\"]`",
))
"#,
            );
        }

        #[test]
        fn empty_issues_format_as_empty_text() {
            assert_schema(r#"#assert.eq(format-issues(()), "")"#);
        }

        #[test]
        fn default_failures_render_as_declared_defaults() {
            assert_schema(
                r#"
#let field = optional(array-of(schema((count: int))), default: ((count: "bad"),))
#let cause = try-parse((:), schema((rows: field))).issues.first()
// The failing default wraps the child issue, and the child path is relative to the default.
#assert.eq(cause.code, "schema.default")
#assert.eq(cause.path, ("rows",))
#assert.eq(cause.children.map(problem => (problem.code, problem.path)), (
  ("schema.type", (0, "count")),
))
#assert(format-issues((cause,)).contains("declared default"))
"#,
            );
        }

        #[test]
        fn output_failures_render_as_converted_values() {
            assert_schema(
                r#"
#let converted = map(str, _ => (count: "bad"), output: schema((count: int)))
#let cause = try-parse((value: "raw"), schema((value: converted))).issues.first()
// A rejected output schema wraps the child issue, whose path is relative to the converted value.
#assert.eq(cause.code, "schema.output")
#assert.eq(cause.path, ("value",))
#assert.eq(cause.children.map(problem => (problem.code, problem.path)), (
  ("schema.type", ("count",)),
))
#assert(format-issues((cause,)).contains("converted value"))
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
        fn explicit_notes_follow_member_issues() {
            assert_schema(
                r#"
#let count = check(int, value => (issue("too small", notes: ("Member count",)),))
#let failed = try-parse((counts: (1,)), schema((counts: array-of(describe(count, "count documentation")))))
#let problem = failed.issues.first()
#assert.eq(problem.path, ("counts", 0))
#assert.eq(problem.notes, ("Member count",))
#assert(format-issues(failed.issues).contains("note: Member count"))
"#,
            );
            for notes in ["none", "7", "(1,)"] {
                reject_schema(&format!(
                    "#let problem = issue(\"Rejected\", notes: {notes})"
                ));
            }
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
        fn relations_do_not_skip_explicit_none() {
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
        fn tuple_checks_members_on_arity_error() {
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
        fn tuple_rest_uses_the_declared_schema() {
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
        fn lazy_factories_wait_for_their_node() {
            assert_schema(
                r#"
#let deferred = lazy(() => panic("absent recursive node was evaluated"))
#assert.eq(parse((:), schema((node: optional(deferred)))), (:))
"#,
            );
        }

        #[test]
        fn variant_runs_only_the_tagged_branch() {
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
        fn one_or_many_never_retries_arrays() {
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
        fn scalar_failures_have_no_array_index() {
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

        #[test]
        fn email_rules_bound_unquoted_addresses() {
            assert_schema(
                r#"
#for address in ("a+b.o'hara@EXAMPLE.test", "a@b", "a" * 64 + "@example.test") {
  assert.eq(parse(address, email), address)
}
#for address in (
  ".a@example.test", "a.@example.test", "a..b@example.test",
  "a" * 65 + "@example.test", "a@" + "b" * 64,
  "a@" + ("b" * 63 + ".") * 4 + "test",
  "é@example.test", "\"a\"@example.test", "Name <a@example.test>",
  "a(comment)@example.test", "a@[192.0.2.1]", " a@example.test",
  "a@example.test ", "a@example.test.", "a@-example.test", "a@example-.test",
) {
  let failed = try-parse(address, email)
  assert.eq(failed.ok, false, message: address)
  assert.eq(failed.issues.first().code, "schema.email")
}
"#,
            );
        }

        #[test]
        fn ipv4_rules_check_every_octet() {
            assert_schema(
                r#"
#for address in ("0.0.0.0", "255.255.255.255", "192.0.2.1") {
  assert.eq(parse(address, ipv4), address)
  assert.eq(parse(address, ip), address)
}
#for address in (
  "01.2.3.4", "256.0.0.1", "1.2.3.256", "1.2.3", "1.2.3.4.5",
  "999999999999999999999999.2.3.4", "+1.2.3.4", "-1.2.3.4",
  "１.2.3.4", "1.2.3.4:80", " 1.2.3.4", "1.2.3.4 ",
) {
  assert.eq(try-parse(address, ipv4).issues.first().code, "schema.ipv4")
  assert.eq(try-parse(address, ip).issues.first().code, "schema.ip")
}
"#,
            );
        }

        #[test]
        fn ipv6_rules_expand_compressed_groups() {
            assert_schema(
                r#"
#for address in (
  "::", "::1", "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7::",
  "2001:db8::a:B", "::ffff:192.0.2.128", "1:2:3:4:5:6:192.0.2.128",
) {
  assert.eq(parse(address, ipv6), address)
  assert.eq(parse(address, ip), address)
}
#for address in (
  ":::", "1::2::3", "1:2:3:4:5:6:7:8::", "1:2:3:4:5:6::192.0.2.1",
  "192.0.2.1::", "::ffff:256.0.2.1", "::ffff:192.00.2.1", "[::1]",
  "fe80::1%en0", "::1/128", "1:2:3:4:5:6:7:", " ::1", "gggg::1",
) {
  assert.eq(try-parse(address, ipv6).issues.first().code, "schema.ipv6")
  assert.eq(try-parse(address, ip).issues.first().code, "schema.ip")
}
"#,
            );
        }
        #[test]
        fn http_schemes_ignore_ascii_case() {
            assert_schema(
                r#"
#for address in ("HTTP://host", "hTtPs://host") {
  assert.eq(parse(address, http-url), address)
}
#assert.eq(parse("HTTPS://host", https-url), "HTTPS://host")
#assert.eq(try-parse("http://host", https-url).issues.first().code, "schema.https-url")
#for address in ("httpſ://host", "ftp://host", "//host", "/relative") {
  assert.eq(try-parse(address, http-url).issues.first().code, "schema.http-url")
}
"#,
            );
        }

        #[test]
        fn url_authorities_keep_valid_literals() {
            assert_schema(
                r#"
#for address in (
  "http://user:p%40ss@[2001:db8::1]:65535/a",
  "http://[::ffff:192.0.2.1]/", "http://[vF.a:b]/", "http://@host/",
  "http://local_host/a", "http://sub!name/", "http://%65xample.test/",
) {
  assert.eq(parse(address, http-url), address)
}
"#,
            );
        }

        #[test]
        fn url_hosts_require_valid_authority() {
            assert_schema(
                r#"
#for address in (
  "http://", "http:///a", "http://@/", "http://a@@b/", "http://[::1",
  "http://[:::]/", "http://[::1]extra/", "http://::1/",
  "http://[v1.]/", "http://[vG.x]/", "http://[fe80::1%25en0]/",
) {
  assert.eq(try-parse(address, http-url).issues.first().code, "schema.http-url")
}
"#,
            );
        }

        #[test]
        fn url_ports_follow_http_range() {
            assert_schema(
                r#"
#for port in ("", "0", "65535", "00065535", "00080") {
  let address = "http://host:" + port + "/"
  assert.eq(parse(address, http-url), address)
}
#for port in ("65536", "00065536", "-1", "+80", "abc", "８０", "999999999999999999999999") {
  assert.eq(try-parse("http://host:" + port + "/", http-url).issues.first().code, "schema.http-url")
}
"#,
            );
        }

        #[test]
        fn urls_reject_invalid_component_text() {
            assert_schema(
                r#"
#for suffix in ("/%", "/%2G", "/<x>", "/a\\b", "/a b", "/\n", "/\u{7f}", "/\u{a0}", "/#one#two") {
  assert.eq(try-parse("http://host" + suffix, http-url).issues.first().code, "schema.http-url")
}
#assert.eq(parse("http://host/%20/%00/%FF", http-url), "http://host/%20/%00/%FF")
#assert.eq(try-parse(none, http-url).issues.first().code, "schema.type")
"#,
            );
        }

        #[test]
        fn unicode_urls_preserve_components() {
            assert_schema(
                r#"
#let address = "HTTPS://例え.テスト/路径?q=%20#片段"
#assert.eq(parse(address, http-url), address)
#assert.eq(parse(address, https-url), address)
#assert.eq(parse("http://host?\u{e000}", http-url), "http://host?\u{e000}")
#for address in ("http://\u{e000}/", "http://host/\u{e000}", "http://host/#\u{e000}") {
  assert.eq(try-parse(address, http-url).issues.first().code, "schema.http-url")
}
"#,
            );
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
        fn batch_issues_anchor_to_each_source() {
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
        fn generated_errors_anchor_to_declarations() {
            let errors = source_errors(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema, optional, map
#let declaration = schema((
  fallback: optional(schema((count: int)), default: (count: "bad")),
  generated: map(str, _ => (count: "bad"), output: schema((count: int))),
))
#let parsed = parse-sources(all-sources(), declaration)
#document("index.html")[]
"#,
                &[(
                    "note.typ",
                    "\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((generated: \"raw\"))",
                )],
            );
            assert_eq!(
                errors
                    .iter()
                    .map(|error| (error.location.path.as_deref(), error.location.line))
                    .collect::<Vec<_>>(),
                [
                    (Some("content/note.typ"), Some(4)),
                    (Some("content/note.typ"), Some(4)),
                ]
            );
        }

        #[test]
        fn malformed_source_records_report_contract() {
            for (sources, expected) in [
                ("none", "parse-sources expects an array"),
                ("(1,)", "value at index 0"),
                ("((path: 1, meta: none),)", "value at index 0"),
            ] {
                let errors = source_errors(
                    &format!(
                        r#"
#import "@tola/source:0.0.0": parse-sources
#import "@tola/schema:0.0.0": schema
#let parsed = parse-sources({sources}, schema((:)))
#document("index.html")[]
"#
                    ),
                    &[],
                );
                assert!(
                    errors.iter().any(|error| error.message.contains(expected)),
                    "{sources}: {errors:?}"
                );
            }
        }

        #[test]
        fn parsed_source_metadata_must_be_dictionary() {
            let errors = source_errors(
                r#"
#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": map
#let parsed = parse-sources(all-sources(), map(dictionary, _ => "not metadata"))
#document("index.html")[]
"#,
                &[(
                    "note.typ",
                    "\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Note\"))",
                )],
            );
            assert_eq!(
                errors
                    .iter()
                    .map(|error| (error.location.path.as_deref(), error.location.line))
                    .collect::<Vec<_>>(),
                [(Some("content/note.typ"), Some(3))]
            );
        }
    }
}
