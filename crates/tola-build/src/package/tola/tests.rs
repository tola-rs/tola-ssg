mod assets;
mod collection;
mod references;
mod schema;
mod source_schema;

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
        modules: Dict::new(),
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
            file: declared.file,
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
    let asset_urls = crate::asset::build_configured_assets(
        &config,
        crate::modules::prepared::AppliedAssets::project_only(&config),
        &crate::resources::source_boundary(&config, tola_input::InputScope::Online),
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
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
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
fn helper_failure_keeps_call_site() {
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
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
        let error = failure.diagnostics().unwrap().errors().next().unwrap();
        assert_eq!(error.location.path.as_deref(), Some("main.typ"));
        assert_eq!(error.location.line, Some(2));
        assert_eq!(
            error.origin == tola_typst::DiagnosticOrigin::Typst,
            is_typst_error
        );
        assert!(
            error.traces.iter().any(|trace| {
                trace.location.path.as_deref() == Some("main.typ") && trace.location.line == Some(3)
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
fn included_source_keeps_identity() {
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
fn qualified_imports_reach_package() {
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
fn datalog_scalars_keep_types() {
    assert_package(
        "datalog",
        r##"
#let keyed = relation("keyed", 2)
#let value = var("value")
#let db = database(facts: (
  keyed(1, "integer"), keyed(1.0, "float"), keyed(decimal("1"), "decimal"),
  keyed("1", "string"), keyed(false, "boolean"), keyed(none, "none"),
))
#assert.eq(query(db, select: (value,), where: (keyed(1, value),)), (("integer",),))
#assert.eq(query(db, select: (value,), where: (keyed(1.0, value),)), (("float",),))
#assert.eq(query(db, select: (value,), where: (keyed(decimal("1"), value),)), (("decimal",),))
#let scalar = relation("scalar", 1)
#let values = (none, true, 7, -2.5, decimal("1.25"), "hello", bytes((0, 255)),
  datetime(year: 2026, month: 11, day: 1), datetime(hour: 12, minute: 30, second: 0))
#let scalars = database(facts: values.map(scalar))
#let rows = query(scalars, select: (value,), where: (scalar(value),))
#assert.eq(rows.len(), values.len())
#for expected in values { assert(rows.contains((expected,))) }
#let zero = database(facts: (scalar(-0.0), scalar(0.0)))
#assert.eq(query(zero, select: (value,), where: (scalar(value),)), ((0.0,),))
"##,
    );
}

#[test]
fn datalog_cached_decimals_keep_scale() {
    assert_package(
        "datalog",
        r##"
#let scalar = relation("scalar", 1)
#let value = var("value")
#for amount in (decimal("1.0"), decimal("1.00")) {
  let db = database(facts: (scalar(amount),))
  let rows = query(db, select: (value,), where: (scalar(value),))
  assert.eq(repr(rows.at(0).at(0)), repr(amount))
}
#let first = decimal("1.0")
#let second = decimal("1.00")
#let db = database(facts: (scalar(first), scalar(second)))
#assert.eq(query(db, select: (), where: (scalar(second),)), ((),))
#let rows = query(db, select: (value,), where: (scalar(value),))
#assert.eq(rows.len(), 1)
#assert.eq(repr(rows.at(0).at(0)), repr(first))
#let pair = relation("pair", 2)
#let db = database(facts: (scalar(first),), rules: (rule(pair(value, value), scalar(value)),))
#for amount in (first, second) {
  let rows = query(db, select: (value,), where: (pair(amount, value),))
  assert.eq(repr(rows.at(0).at(0)), repr(amount))
}
"##,
    );
}

#[test]
fn datalog_snapshots_canonicalize_rows() {
    assert_package(
        "datalog",
        r##"
#let p = relation("p", 1)
#let x = var("x")
#for (left, right, same-left) in ((1, 2, 1), (bytes("a"), bytes("b"), bytes((97,)))) {
  let one = p(left)
  let two = p(right)
  let db = database(facts: (one, two))
  assert.eq(db, database(facts: (two, one, p(same-left))))
  assert.eq(db, database(facts: (p(left), p(right))))
  assert.eq(query(db, select: (x,), where: (p(x),)), ((left,), (right,)))
}
"##,
    );
}

#[test]
fn datalog_queries_use_their_snapshot() {
    assert_package(
        "datalog",
        r##"
#let link = relation("link", 2)
#let reach = relation("reach", 2)
#let x = var("x")
#let y = var("y")
#let z = var("z")
#let rules = (rule(reach(x, y), link(x, y)), rule(reach(x, z), reach(x, y), link(y, z)))
#let before = database(facts: (link("a", "b"), link("b", "c")), rules: rules)
#let after = database(facts: (link("a", "b"),), rules: rules)
#let ask(db) = query(db, select: (z,), where: (reach("a", z),))
#assert.eq(ask(before), (("b",), ("c",)))
#assert.eq(ask(after), (("b",),))
#assert.eq(ask(before), (("b",), ("c",)))
#let changed = ask(before)
#{ changed.at(0).at(0) = "changed" }
#assert.eq(ask(before), (("b",), ("c",)))
#assert.eq(query(after, select: (), where: (reach("a", "b"),)), ((),))
#assert.eq(query(after, select: (), where: (reach("a", "c"),)), ())
"##,
    );
}

#[test]
fn cached_errors_keep_location() {
    for (declarations, clauses, line) in [
        (
            "#let p = d.relation(\"p\", 1)\n#let x = d.var(\"x\")\n",
            "#let r = d.rule(p(x))\n#let db = d.database(rules: (r,))\n",
            3,
        ),
        (
            "#let downstream = d.relation(\"downstream\", 0)\n\
             #let cycle = d.relation(\"cycle\", 0)\n",
            "#let dependent = d.rule(downstream(), cycle())\n\
             #let recursive = d.rule(cycle(), d.negate(cycle()))\n\
             #let db = d.database(rules: (dependent, recursive))\n",
            4,
        ),
        (
            "#let p = d.relation(\"p\", 1)\n",
            "#let x = d.var(\"x\")\n\
             #let rows = d.query(d.database(), select: (x,), where: ())\n",
            2,
        ),
    ] {
        for blank_lines in [0, 3] {
            let source = format!(
                "#import \"@tola/datalog:0.0.0\" as d\n{declarations}{}{clauses}",
                "\n".repeat(blank_lines),
            );
            let (_directory, world) = world_from_default_config(&source, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let error = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(error.location.path.as_deref(), Some("main.typ"));
            assert_eq!(error.location.range.unwrap().start.line, line + blank_lines);
        }
    }
}

#[test]
fn datalog_rejects_invalid_clauses() {
    for expression in [
        r#"relation("", 1)"#,
        r#"relation("p", -1)"#,
        r#"relation("p", 2)(1)"#,
        r#"relation("p", 1)([content])"#,
        r#"relation("p", 1)(float("inf"))"#,
        r#"database(facts: (relation("p", 1)(var("x")),))"#,
        r#"database(facts: (1,))"#,
        r#"database(rules: (1,))"#,
        r#"database(facts: (relation("p", 1)(1), relation("p", 2)(1, 2)))"#,
        r#"database(rules: (rule(relation("p", 1)(var("x"))),))"#,
        r#"query(database(), select: (var("x"),), where: ())"#,
        r#"query(database(), select: (1,), where: ())"#,
        r#"query(database(), select: (), where: (1,))"#,
        r#"query(database(), select: (), where: (negate(relation("p", 1)(var("x"))),))"#,
    ] {
        reject_package("datalog", &format!("#let value = {expression}"));
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
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
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
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
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
                trace.location.path.as_deref() == Some("main.typ") && trace.location.line == Some(3)
            }),
            "{error:?}"
        );
    }
}
