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

use tempfile::TempDir;
use typst::World;
use typst::foundations::{Array, Dict, IntoValue};

use super::*;
use crate::content::{
    ContentId, ContentSourceLayout, ContentUnit, SourceMetadataDeclaration, SourceRecords,
    SourceSet,
};
use crate::metadata::SourceMetadata;
use crate::package::SiteBindings;

#[derive(Default)]
struct IconFiles(Arc<tola_icons::IconCollections>);

impl tola_typst::FileProvider for IconFiles {
    fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
        let typst::syntax::VirtualRoot::Package(pkg) = id.root() else {
            return None;
        };
        let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
        if tola_packages::is_icon_file(pkg, path) {
            return Some(icon_file_target(&self.0, pkg, path));
        }
        read_package(pkg, id.vpath().get_with_slash())
            .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
    }
}

fn typst_world(
    src: &str,
    lib: &tola_packages::library::SiteLibrary,
    icons: Arc<tola_icons::IconCollections>,
    fonts: Option<Arc<FontStore>>,
) -> (TempDir, TypstWorld) {
    let dir = TempDir::new().unwrap();
    let main = dir.path().join("main.typ");
    fs::write(&main, src).unwrap();
    let files = tola_typst::FileStore::new().with_provider(IconFiles(icons));
    let builder = TypstWorld::builder(&main, dir.path())
        .with_files(Arc::new(files))
        .with_local_cache()
        .with_shared_library(lib.shared());
    let builder = match fonts {
        Some(fonts) => builder.with_fonts(fonts),
        None => builder.no_fonts(),
    };
    let world = builder
        .build(&tola_typst::BundleCancellation::default())
        .expect("valid test world");
    (dir, world)
}

fn default_library(srcs: SourceRecords) -> tola_packages::library::SiteLibrary {
    let cfg = crate::config::tests::OwnedSiteConfig::new("");
    SiteBindings::from_config(&cfg.config, Default::default(), None).library(srcs)
}

fn world_from_default_config(src: &str, srcs: SourceRecords) -> (TempDir, TypstWorld) {
    typst_world(src, &default_library(srcs), Arc::default(), None)
}

fn assert_default_compiles(src: &str) {
    let (_dir, world) = world_from_default_config(src, SourceRecords::default());
    tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default()).unwrap();
}

const EXAMPLE_SITE_CONFIG: &str = r#"[site]
title = "Example site"
origin = "https://example.test"

[assets]
[[assets.trees]]
source = "assets"
url-prefix = "/assets"

[icons.collections.brand]
source-type = "local-svg-dir"
path = "icons/brand"
"#;

const MARK_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M12 2 2 22h20z"/></svg>"#;
const LOGO_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="currentColor"/></svg>"#;

fn logo_png() -> Vec<u8> {
    let pixels = ::image::RgbImage::from_pixel(8, 4, ::image::Rgb([0x33, 0x66, 0x99]));
    let mut buf = Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut buf, ::image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

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

fn declared_metadata(src: &Path, root: &Path) -> Option<SourceMetadataDeclaration> {
    let world = TypstWorld::builder(src, root)
        .with_files(Arc::new(
            tola_typst::FileStore::new().with_provider(IconFiles::default()),
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

const EXAMPLE_CONTENT: [(&str, &str); 7] = [
    (
        "index.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Welcome\"))\n",
    ),
    (
        "guide/index.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Guide\", order: 0))\n",
    ),
    (
        "guide/install.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Install\", order: 1))\n",
    ),
    (
        "guide/deploy.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Deploy\", order: 2))\n",
    ),
    (
        "guide/advanced/plugins.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Plugins\", order: 3))\n",
    ),
    (
        "notes/first-post.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"First post\", tags: (\"web\",), published: datetime(year: 2026, month: 5, day: 1)))\n",
    ),
    (
        "notes/second-post.typ",
        "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Second post\", tags: (\"web\", \"typst\"), published: datetime(year: 2026, month: 6, day: 1)))\n",
    ),
];

fn world_from_example_site(src: &str, files: &[(String, String)]) -> (TempDir, TypstWorld) {
    let dir = TempDir::new().unwrap();
    let cfg = crate::config::tests::load_test_config(dir.path(), EXAMPLE_SITE_CONFIG);
    let root = cfg.root();

    let content = root.join("content");
    let mut units = Vec::new();
    let mut decls = BTreeMap::new();
    for (path, text) in EXAMPLE_CONTENT {
        let src_path = content.join(path);
        fs::create_dir_all(src_path.parent().unwrap()).unwrap();
        fs::write(&src_path, text).unwrap();
        decls.insert(src_path.clone(), declared_metadata(&src_path, root));
        units.push(ContentUnit {
            root: content.clone(),
            id: ContentId::new(path.into()),
            source: src_path,
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

    for (path, contents) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    let cancel = crate::cancellation::BuildCancellation::default();
    let collections = crate::icon::prepare(
        &cfg,
        &cancel,
        None,
        &crate::resources::BuildResources::default(),
        &crate::filesystem::SourceOverrides::default(),
    )
    .expect("example site icon collection loads")
    .collections();
    let asset_urls = crate::asset::build_configured_assets(
        &cfg,
        crate::modules::site::AppliedAssets::project_only(&cfg),
        &crate::resources::source_boundary(&cfg, tola_input::InputScope::Online),
        &cancel,
        None,
        &[],
    )
    .expect("example site asset tree renders")
    .asset_urls(&cfg)
    .expect("example site asset URLs resolve");
    let records = SourceSet::without_metadata(&units, &cfg)
        .unwrap()
        .with_metadata(&decls)
        .inputs()
        .to_source_records();
    let lib = SiteBindings::from_config(&cfg, asset_urls, None).library(records);

    let main = root.join("main.typ");
    fs::write(&main, src).unwrap();
    let files = tola_typst::FileStore::new().with_provider(IconFiles(collections));
    let fonts = Arc::new(FontStore::with_options(
        FontOptions::new()
            .with_system_fonts(false)
            .with_embedded_fonts(true),
    ));
    let world = TypstWorld::builder(&main, root)
        .with_files(Arc::new(files))
        .with_local_cache()
        .with_shared_library(lib.shared())
        .with_fonts(fonts)
        .build(&tola_typst::BundleCancellation::default())
        .expect("valid example site world");
    (dir, world)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExampleWorld {
    Default,
    ExampleSite,
}

impl ExampleWorld {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ExampleSite => "example site",
        }
    }
}

struct Example {
    line: usize,
    source: String,
    world: ExampleWorld,
    files: Vec<(String, String)>,
}

enum ExampleFence {
    Typst(ExampleWorld),
    File(String),
    Output,
}

fn documented_examples(docs: &str) -> Result<Vec<Example>, String> {
    let mut examples = Vec::new();
    let mut fence: Option<(Option<u8>, usize, usize, ExampleFence)> = None;
    let mut src = String::new();
    let mut files = Vec::new();
    for (idx, line) in docs.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied();
        let len = trimmed.bytes().take_while(|b| Some(*b) == marker).count();
        let is_fence = matches!(marker, Some(b'`' | b'~')) && len >= 3;
        if let Some((opening, min, start, kind)) = &fence {
            if is_fence && marker == *opening && len >= *min && trimmed[len..].trim().is_empty() {
                let contents = std::mem::take(&mut src);
                match kind {
                    ExampleFence::Typst(world) => {
                        if *world == ExampleWorld::Default && !files.is_empty() {
                            return Err(format!(
                                "files before line {start} need a `typst site` fence"
                            ));
                        }
                        examples.push(Example {
                            line: *start,
                            source: contents,
                            world: *world,
                            files: std::mem::take(&mut files),
                        });
                    }
                    ExampleFence::File(path) => files.push((path.clone(), contents)),
                    ExampleFence::Output => {}
                }
                fence = None;
            } else if !matches!(kind, ExampleFence::Output) {
                src.push_str(line);
                src.push('\n');
            }
        } else if is_fence {
            let info = trimmed[len..].trim();
            let kind = match info.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["typ" | "typst" | "typc"] => ExampleFence::Typst(ExampleWorld::Default),
                ["typ" | "typst" | "typc", "site"] => {
                    ExampleFence::Typst(ExampleWorld::ExampleSite)
                }
                ["html"] => ExampleFence::Output,
                [fmt @ ("svg" | "json"), path]
                    if Path::new(path).extension().and_then(|e| e.to_str()) == Some(fmt)
                        && Path::new(path)
                            .components()
                            .all(|p| matches!(p, std::path::Component::Normal(_))) =>
                {
                    ExampleFence::File((*path).to_owned())
                }
                _ => {
                    return Err(format!(
                        "invalid API example fence at documentation line {}: `{info}`",
                        idx + 1
                    ));
                }
            };
            fence = Some((marker, len, idx + 1, kind));
        }
    }
    if let Some((_, _, start, _)) = fence {
        return Err(format!(
            "unclosed API example fence at documentation line {start}"
        ));
    }
    if !files.is_empty() {
        return Err("example files must be followed by a `typst site` fence".to_owned());
    }
    Ok(examples)
}

#[test]
fn documented_examples_compile() {
    let mut checked = 0;
    for pkg in tola_packages::builtin_packages() {
        let names: Vec<_> = pkg.exports().map(str::to_owned).collect();
        let mut sections = vec![("overview".to_owned(), pkg.overview())];
        sections.extend(
            pkg.export_documentation(&names)
                .unwrap()
                .into_iter()
                .map(|e| {
                    let summary = e.documentation.summary.clone();
                    (e.name, (!summary.is_empty()).then_some(summary))
                }),
        );
        for (name, docs) in sections {
            let Some(docs) = docs else { continue };
            let examples = documented_examples(&docs)
                .unwrap_or_else(|err| panic!("{} / {name}: {err}", pkg.spec()));
            for (
                idx,
                Example {
                    line,
                    source,
                    world,
                    files,
                },
            ) in examples.into_iter().enumerate()
            {
                let (_dir, comp_world) = match world {
                    ExampleWorld::Default => {
                        world_from_default_config(&source, SourceRecords::default())
                    }
                    ExampleWorld::ExampleSite => world_from_example_site(&source, &files),
                };
                let comp = tola_typst::compile_bundle_world(&comp_world, &Default::default());
                assert!(
                    comp.is_ok(),
                    "{} / {name} / example {} (documentation line {line}) in the {} world\n{source}\n{}",
                    pkg.spec(),
                    idx + 1,
                    world.name(),
                    comp.unwrap_err(),
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no public API examples were discovered");
}

fn package_program(pkg: &str, assertions: &str) -> String {
    format!(
        "#import \"@tola/{pkg}:0.0.0\": *\n\
         {assertions}\n\
         #document(\"assertions.html\")[]"
    )
}

fn package_world(src: &str) -> (TempDir, TypstWorld) {
    let dir = TempDir::new().unwrap();
    let main = dir.path().join("main.typ");
    fs::write(&main, src).unwrap();
    let files = tola_typst::FileStore::new().with_provider(IconFiles::default());
    let world = TypstWorld::builder(&main, dir.path())
        .with_files(Arc::new(files))
        .with_local_cache()
        .no_fonts()
        .with_shared_library(source_library())
        .build(&tola_typst::BundleCancellation::default())
        .expect("valid test world");
    (dir, world)
}

fn assert_package(pkg: &str, assertions: &str) {
    let (_dir, world) = package_world(&package_program(pkg, assertions));
    tola_typst::compile_bundle_world(&world, &Default::default())
        .expect("package assertions must hold");
}

fn reject_package(pkg: &str, assertions: &str) {
    let (_dir, world) = package_world(&package_program(pkg, assertions));
    tola_typst::compile_bundle_world(&world, &Default::default())
        .expect_err("invalid assertions must fail compiling")
        .diagnostics()
        .expect("Typst failure has diagnostics");
}

fn source_records(ids: &[&str]) -> SourceRecords {
    let cfg = crate::config::tests::OwnedSiteConfig::new("");
    let root = cfg.config.root().join("content");
    let units: Vec<_> = ids
        .iter()
        .map(|id| ContentUnit {
            root: root.clone(),
            id: ContentId::new((*id).into()),
            source: root.join(id),
            layout: ContentSourceLayout::from_entry_path(Path::new(id)),
        })
        .collect();
    SourceSet::without_metadata(&units, &cfg.config)
        .unwrap()
        .inputs()
        .to_source_records()
}

#[test]
fn address_error_spans() {
    for (func, arg) in [
        ("output-of", "relative"),
        ("route-of", "/index.html"),
        ("decode-url-path", "/%zz"),
    ] {
        let src = format!(
            "#import \"@tola/address:0.0.0\": {func}\n\
             #let url = {func}({arg:?})\n\
             #document(\"index.html\")[]"
        );
        let (_dir, world) = world_from_default_config(&src, SourceRecords::default());
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
        let err = failure.diagnostics().unwrap().errors().next().unwrap();
        assert_eq!(err.location.path.as_deref(), Some("main.typ"));
        let range = err.location.range.unwrap();
        let line = src.lines().nth(range.start.line).unwrap();
        assert_eq!(range.start.line, range.end.line);
        assert_eq!(
            line[range.start.character..range.end.character].trim_matches('"'),
            arg,
            "{err:?}"
        );
    }
}

#[test]
fn helper_error_call_site() {
    for (helper, is_typst_err) in [
        (
            r#"#let url() = address("/guide/", base-path: "/docs")"#,
            false,
        ),
        (r#"#let url() = address(panic("route unavailable"))"#, true),
    ] {
        let src = format!(
            "#import \"@tola/address:0.0.0\": route-url as address\n\
             {helper}\n\
             #let selected = url()\n\
             #document(\"index.html\")[]"
        );
        let (_dir, world) = world_from_default_config(&src, SourceRecords::default());
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
        let err = failure.diagnostics().unwrap().errors().next().unwrap();
        assert_eq!(err.location.path.as_deref(), Some("main.typ"));
        assert_eq!(err.location.line, Some(2));
        assert_eq!(
            err.origin == tola_typst::DiagnosticOrigin::Typst,
            is_typst_err
        );
        assert!(
            err.traces.iter().any(|t| {
                t.location.path.as_deref() == Some("main.typ") && t.location.line == Some(3)
            }),
            "{err:?}"
        );
    }
}

#[test]
fn url_path_schema_decodes() {
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
fn helper_current_source() {
    let srcs = source_records(&["helper.typ"]);
    let (dir, world) = world_from_default_config(
        r#"
#import "content/helper.typ": helper-source
#assert.eq(helper-source().filename, "helper.typ")
#assert.eq(helper-source().path, "helper.typ")
#document("index.html")[]
"#,
        srcs,
    );
    fs::create_dir(dir.path().join("content")).unwrap();
    fs::write(
        dir.path().join("content/helper.typ"),
        "#import \"@tola/source:0.0.0\": current-source\n#let helper-source() = current-source()\n",
    )
    .unwrap();
    let comp = tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
    assert_eq!(comp.documents().count(), 1);
}

#[test]
fn included_source_identity() {
    let srcs = source_records(&["document.typ"]);
    let (dir, world) = world_from_default_config(
        r#"
#import "@tola/source:0.0.0": all-sources
#let document-source = all-sources().first()
#document("first/index.html")[#include document-source.file]
#document("second.html")[#include document-source.file]
"#,
        srcs,
    );
    fs::create_dir(dir.path().join("content")).unwrap();
    fs::write(
        dir.path().join("content/document.typ"),
        r#"
#import "@tola/address:0.0.0": join-route
#import "@tola/document:0.0.0": current-document
#import "@tola/source:0.0.0": current-source, tola-meta
#let source = current-source()
#assert.eq(source.path, "document.typ")
#assert.eq(join-route(source.at("route-segments")), "/document/")
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
    let comp = tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
    assert_eq!(comp.documents().count(), 2);
}

#[test]
fn qualified_imports() {
    let srcs = source_records(&["document.typ"]);
    let (dir, world) = world_from_default_config(
        r#"
#import "@tola/source:0.0.0"
#import "@tola/head:0.0.0" as page-head
#import "@tola/document:0.0.0" as doc
#let document-source = source.all-sources().first()
#assert.eq(document-source.filename, "document.typ")
#assert.eq(doc.plain-text([Native *title*]), "Native title")
#assert.eq(page-head.head-metadata((:)).len(), 2)
#document("first/index.html")[#include document-source.file]
#document("second.html")[#include document-source.file]
#document("native.html")[
  #html.html[
    #html.head(page-head.head-metadata((title: [Native *title*])).join())
    #html.body[#context assert.eq(doc.current-document().output, "native.html")]
  ]
]
"#,
        srcs,
    );
    fs::create_dir(dir.path().join("content")).unwrap();
    fs::write(
        dir.path().join("content/document.typ"),
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
    let comp = tola_typst::compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
    assert_eq!(comp.documents().count(), 3);
}

#[test]
fn datalog_values_and_decimals() {
    assert_package(
        "datalog",
        r##"
#let keyed = relation("keyed", 2)
#let value = var("value")
#let db = database((keyed,), facts: (
  keyed.atom(1, "integer"), keyed.atom(1.0, "float"), keyed.atom(decimal("1"), "decimal"),
  keyed.atom("1", "string"), keyed.atom(false, "boolean"), keyed.atom(none, "none"),
))
#assert.eq(db.query(select: (value,), where: (keyed.atom(1, value),)), (("integer",),))
#assert.eq(db.query(select: (value,), where: (keyed.atom(1.0, value),)), (("float",),))
#assert.eq(db.query(select: (value,), where: (keyed.atom(decimal("1"), value),)), (("decimal",),))
#let scalar = relation("scalar", 1)
#let values = (none, true, 7, -2.5, decimal("1.25"), "hello", bytes((0, 255)),
  datetime(year: 2026, month: 11, day: 1), datetime(hour: 12, minute: 30, second: 0))
#let scalars = database((scalar,), facts: values.map(it => scalar.atom(it)))
#let rows = scalars.query(select: (value,), where: (scalar.atom(value),))
#assert.eq(rows.len(), values.len())
#for expected in values { assert(rows.contains((expected,))) }
#let zero = database((scalar,), facts: (scalar.atom(-0.0), scalar.atom(0.0)))
#assert.eq(zero.query(select: (value,), where: (scalar.atom(value),)), ((0.0,),))

#for (amount, canonical) in (
  (decimal("1.0"), decimal("1")),
  (decimal("1.00"), decimal("1")),
  (decimal("-8.700"), decimal("-8.7")),
  (decimal("0.00"), decimal("0")),
) {
  let db = database((scalar,), facts: (scalar.atom(amount),))
  let rows = db.query(select: (value,), where: (scalar.atom(value),))
  assert.eq(repr(rows.at(0).at(0)), repr(canonical))
}
#let first = decimal("1.0")
#let second = decimal("1.00")
#let db = database((scalar,), facts: (scalar.atom(first), scalar.atom(second)))
#assert.eq(db, database((scalar,), facts: (scalar.atom(second), scalar.atom(first))))
#assert.eq(db.query(select: (), where: (scalar.atom(second),)), ((),))
#let rows = db.query(select: (value,), where: (scalar.atom(value),))
#assert.eq(rows.len(), 1)
#assert.eq(repr(rows.at(0).at(0)), repr(decimal("1")))
#let pair = relation("pair", 2)
#let db = database((scalar, pair), facts: (scalar.atom(first),), rules: (rule(pair.atom(value, value), scalar.atom(value)),))
#for amount in (first, second) {
  let rows = db.query(select: (value,), where: (pair.atom(amount, value),))
  assert.eq(repr(rows.at(0).at(0)), repr(decimal("1")))
}
"##,
    );
}

#[test]
fn datalog_snapshots_and_queries() {
    assert_package(
        "datalog",
        r##"
#let p = relation("p", 1)
#let x = var("x")
#for (left, right, same-left) in ((1, 2, 1), (bytes("a"), bytes("b"), bytes((97,)))) {
  let one = p.atom(left)
  let two = p.atom(right)
  let db = database((p,), facts: (one, two))
  assert.eq(db, database((p,), facts: (two, one, p.atom(same-left))))
  assert.eq(db, database((p,), facts: (p.atom(left), p.atom(right))))
  assert.eq(db.query(select: (x,), where: (p.atom(x),)), ((left,), (right,)))
}

#let link = relation("link", 2)
#let reach = relation("reach", 2)
#let y = var("y")
#let z = var("z")
#let rules = (rule(reach.atom(x, y), link.atom(x, y)), rule(reach.atom(x, z), reach.atom(x, y), link.atom(y, z)))
#let before = database((link, reach), facts: (link.atom("a", "b"), link.atom("b", "c")), rules: rules)
#let after = database((link, reach), facts: (link.atom("a", "b"),), rules: rules)
#let ask(db) = db.query(select: (z,), where: (reach.atom("a", z),))
#assert.eq(ask(before), (("b",), ("c",)))
#assert.eq(ask(after), (("b",),))
#assert.eq(ask(before), (("b",), ("c",)))
#let changed = ask(before)
#{ changed.at(0).at(0) = "changed" }
#assert.eq(ask(before), (("b",), ("c",)))
#assert.eq(after.query(select: (), where: (reach.atom("a", "b"),)), ((),))
#assert.eq(after.query(select: (), where: (reach.atom("a", "c"),)), ())
#assert.eq(after.query(select: (), where: ()), ((),))
#assert(database(()) != database((link,)))
#let empty-link = relation("link", 2)
#assert.eq(database((link,)).query(select: (), where: (negate(empty-link.atom("a", "b")),)), ((),))
"##,
    );
}

#[test]
fn datalog_error_locations() {
    for (decls, clauses, line) in [
        (
            "#let p = d.relation(\"p\", 1)\n#let x = d.var(\"x\")\n",
            "#let r = d.rule(p.atom(x), p.atom(1))\n#let db = d.database((p,), rules: (r,))\n",
            3,
        ),
        (
            "#let downstream = d.relation(\"downstream\", 0)\n#let cycle = d.relation(\"cycle\", 0)\n",
            "#let dependent = d.rule(downstream.atom(), cycle.atom())\n#let recursive = d.rule(cycle.atom(), d.negate(cycle.atom()))\n#let db = d.database((downstream, cycle), rules: (dependent, recursive))\n",
            4,
        ),
        (
            "#let p = d.relation(\"p\", 1)\n",
            "#let x = d.var(\"x\")\n#let rows = d.database((p,)).query(select: (x,), where: ())\n",
            2,
        ),
    ] {
        for blank_lines in [0, 3] {
            let src = format!(
                "#import \"@tola/datalog:0.0.0\" as d\n{decls}{}{clauses}",
                "\n".repeat(blank_lines),
            );
            let (_dir, world) = world_from_default_config(&src, SourceRecords::default());
            let failure =
                tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
            let err = failure.diagnostics().unwrap().errors().next().unwrap();
            assert_eq!(err.location.path.as_deref(), Some("main.typ"));
            assert_eq!(err.location.range.unwrap().start.line, line + blank_lines);
        }
    }
}

#[test]
fn datalog_invalid_clauses() {
    for expr in [
        r#"relation("", 1)"#,
        r#"relation("p", -1)"#,
        r#"var("")"#,
        r#"relation("p", 2).atom(1)"#,
        r#"relation("p", 1).atom([content])"#,
        r#"relation("p", 1).atom(float("inf"))"#,
        r#"database((1,))"#,
        r#"database((relation("p", 1), relation("p", 1)))"#,
        r#"database((relation("p", 1),), facts: (relation("p", 1).atom(var("x")),))"#,
        r#"database((), facts: (1,))"#,
        r#"database((), rules: (1,))"#,
        r#"database((relation("p", 1),), facts: (relation("p", 2).atom(1, 2),))"#,
        r#"rule(relation("p", 0).atom())"#,
        r#"database((relation("p", 0),), rules: (rule(relation("p", 0).atom(), negate(relation("missing", 0).atom())),))"#,
        r#"database(()).query(where: ())"#,
        r#"database(()).query(select: ())"#,
        r#"database(()).query(select: none, where: ())"#,
        r#"database(()).query(select: (var("x"),), where: ())"#,
        r#"database(()).query(select: (1,), where: ())"#,
        r#"database(()).query(select: (), where: (1,))"#,
        r#"database((relation("p", 1),)).query(select: (), where: (negate(relation("p", 1).atom(var("x"))),))"#,
        r#"database(()).query(select: (), where: (negate(relation("missing", 0).atom()),))"#,
    ] {
        reject_package("datalog", &format!("#let value = {expr}"));
    }
}

#[test]
fn package_guard_spans() {
    for (pkg, expr, details) in [
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
        let src = format!(
            "#import \"@tola/{pkg}:0.0.0\": *\n\
             #let selected = {expr}\n\
             #document(\"index.html\")[]"
        );
        let (_dir, world) = world_from_default_config(&src, SourceRecords::default());
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
        let err = failure.diagnostics().unwrap().errors().next().unwrap();
        assert_eq!(err.location.path.as_deref(), Some("main.typ"));
        let range = err.location.range.unwrap();
        assert_eq!((range.start.line, range.end.line), (1, 1), "{err:?}");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(
            &line[range.start.character..range.end.character],
            expr,
            "{err:?}"
        );
        assert!(matches!(
            err.origin,
            tola_typst::DiagnosticOrigin::Producer(_)
        ));
        for detail in details {
            assert!(err.message.contains(detail), "{expr}: {err:?}");
        }
        assert!(!err.message.contains("assertion failed"), "{err:?}");
        assert!(!err.message.contains("panicked"), "{err:?}");
    }
}

#[test]
fn package_callback_spans() {
    for (pkg, expr) in [
        ("collection", r#"index-by(("a",), key: fail)"#),
        ("schema", "parse(1, map(int, fail))"),
    ] {
        let src = format!(
            "#import \"@tola/{pkg}:0.0.0\": *\n\
             #let fail(value) = panic(\"callback unavailable\")\n\
             #let selected = {expr}\n\
             #document(\"index.html\")[]"
        );
        let (_dir, world) = world_from_default_config(&src, SourceRecords::default());
        let failure = tola_typst::compile_bundle_world(&world, &Default::default()).unwrap_err();
        let err = failure.diagnostics().unwrap().errors().next().unwrap();
        assert_eq!(err.location.path.as_deref(), Some("main.typ"));
        let range = err.location.range.unwrap();
        assert_eq!((range.start.line, range.end.line), (1, 1), "{err:?}");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(
            &line[range.start.character..range.end.character],
            r#"panic("callback unavailable")"#,
            "{err:?}"
        );
        assert_eq!(err.origin, tola_typst::DiagnosticOrigin::Typst);
        assert!(err.message.contains("callback unavailable"), "{err:?}");
        assert!(
            err.traces.iter().any(|t| {
                t.location.path.as_deref() == Some("main.typ") && t.location.line == Some(3)
            }),
            "{err:?}"
        );
    }
}
