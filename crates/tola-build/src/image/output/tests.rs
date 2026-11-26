use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_address::OutputPath;
use tola_input::layout;
use tola_typst::ContentDigest;

use crate::build::{
    BuildMode, BuildRequest, BuildReuse, BuildSession, BuildTrigger, RevisionCheck, SiteBuild,
};
use crate::cancellation::BuildCancellation;
use crate::config::SiteConfig;
use crate::config::tests::OwnedSiteConfig;

fn cfg_with_program(program: &str) -> OwnedSiteConfig {
    let site_cfg = OwnedSiteConfig::new("[site]\nbase-path = '/docs/'\n");
    fs::create_dir_all(&site_cfg.config.build.content_dir).unwrap();
    fs::write(&site_cfg.config.build.entry, program).unwrap();
    site_cfg
}

fn png_bytes(color: [u8; 3]) -> Vec<u8> {
    let px = ::image::RgbImage::from_pixel(8, 4, ::image::Rgb(color));
    let mut buf = Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgb8(px)
        .write_to(&mut buf, ::image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

fn png(path: &Path, color: [u8; 3]) {
    fs::write(path, png_bytes(color)).unwrap();
}

fn svg(path: &Path, w: u32, h: u32, body: &str) {
    fs::write(
        path,
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">{body}</svg>"##
        ),
    )
    .unwrap();
}

/// Remove IDAT payload so inspecting headers succeeds while decoding pixels fails.
fn strip_pixel_data(path: &Path) {
    let bytes = fs::read(path).unwrap();
    let chunk = bytes
        .windows(4)
        .position(|w| w == b"IDAT")
        .expect("an encoded PNG has pixel data");
    let len = u32::from_be_bytes(bytes[chunk - 4..chunk].try_into().unwrap()) as usize;
    let mut stripped = Vec::new();
    stripped.extend_from_slice(&bytes[..chunk - 4]);
    stripped.extend_from_slice(&0u32.to_be_bytes());
    stripped.extend_from_slice(b"IDAT");
    stripped.extend_from_slice(&bytes[chunk + 4 + len..]);
    fs::write(path, stripped).unwrap();
}

fn request(paths: Vec<PathBuf>) -> BuildRequest {
    BuildRequest {
        mode: BuildMode::Development,
        trigger: BuildTrigger::Paths(paths.into()),
        reuse: BuildReuse {
            content_inventory: true,
            typst_compilation: true,
            configured_assets: true,
        },
        ..BuildRequest::new(BuildMode::Development)
    }
}

fn hashes(build: &SiteBuild) -> BTreeMap<OutputPath, ContentDigest> {
    build
        .graph()
        .outputs()
        .iter()
        .map(|out| (out.path().clone(), ContentDigest::of(out.bytes())))
        .collect()
}

fn install(session: &mut BuildSession, build: SiteBuild) {
    let checked = match build
        .into_unchecked_revision(None)
        .check(&BuildCancellation::new())
        .unwrap()
    {
        RevisionCheck::Fresh(checked) => checked,
        RevisionCheck::Stale(_) => panic!("test candidate unexpectedly became stale"),
    };
    session.install_revision(checked, Ok).unwrap();
}

fn cold_build(cfg: Arc<SiteConfig>) -> SiteBuild {
    BuildSession::new()
        .prepare(cfg, BuildRequest::new(BuildMode::Development))
        .run()
        .unwrap()
}

fn stored_variants(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let dir = layout::image_cache(root);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter_map(|ptr| {
            let name = fs::read_to_string(&ptr).ok()?;
            let stored = dir.join(name.trim());
            stored.is_file().then_some((ptr, stored))
        })
        .collect()
}

fn damage_stored_variants(root: &Path) {
    for (_ptr, stored) in stored_variants(root) {
        fs::write(stored, b"damaged").unwrap();
    }
}

fn one_image_cfg() -> OwnedSiteConfig {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("photo.png", width: 4, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: resized.url, alt: "Photo")]"#,
    );
    png(&site_cfg.config.root().join("photo.png"), [30, 60, 90]);
    site_cfg
}

fn derivative_outputs(build: &SiteBuild) -> Vec<&crate::output::graph::OutputFile> {
    build
        .graph()
        .outputs()
        .iter()
        .filter(|out| out.path().as_str().starts_with("_tola/images/"))
        .collect()
}

fn derivative_paths(build: &SiteBuild) -> Vec<String> {
    derivative_outputs(build)
        .iter()
        .map(|out| out.path().as_str().to_owned())
        .collect()
}

fn only_diagnostic(site_cfg: &OwnedSiteConfig) -> tola_diagnostic::Diagnostic {
    let err = match crate::build::build_site(&site_cfg.config, BuildMode::Production) {
        Err(err) => err,
        Ok(_) => panic!("the build must fail"),
    };
    let diags = err.diagnostics(site_cfg.config.root());
    let [diag] = diags.as_slice() else {
        panic!("expected one diagnostic: {diags:#?}")
    };
    diag.clone()
}

fn page_text<'a>(build: &'a SiteBuild, name: &str) -> &'a str {
    let page = build
        .graph()
        .outputs()
        .iter()
        .find(|out| out.path().as_str() == name)
        .expect("the build published the page");
    std::str::from_utf8(page.bytes()).unwrap()
}

#[test]
fn build_stores_variants() {
    let site_cfg = one_image_cfg();
    let cfg = Arc::new(site_cfg.config.clone());
    let built = cold_build(Arc::clone(&cfg));
    let published = derivative_outputs(&built)
        .first()
        .expect("the build published the resized image")
        .bytes()
        .to_vec();
    let stored = stored_variants(cfg.root());
    assert_eq!(stored.len(), 1, "the build stored the variant it published");
    assert_eq!(fs::read(&stored[0].1).unwrap(), published);
}

#[test]
fn srcset_distinct_widths() {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("photo.png", widths: (4, 2, 4))
#document("index.html")[
  #html.elem("img", attrs: (
    src: photo.url, srcset: photo.srcset, width: str(photo.width), height: str(photo.height), alt: "Photo",
  ))
]"#,
    );
    png(&site_cfg.config.root().join("photo.png"), [30, 60, 90]);
    let built = cold_build(Arc::new(site_cfg.config.clone()));
    let derivatives = derivative_paths(&built);
    assert_eq!(
        derivatives.len(),
        2,
        "one derivative per distinct width: {derivatives:#?}"
    );
    let page = page_text(&built, "index.html");
    assert_eq!(page.matches(" 2w").count(), 1, "{page}");
    assert_eq!(page.matches(" 4w").count(), 1, "{page}");

    let impossible = cfg_with_program(
        r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("photo.png", widths: (4, 0))
#document("index.html")[#html.img(src: photo.url, alt: "Photo")]"#,
    );
    png(&impossible.config.root().join("photo.png"), [30, 60, 90]);
    let diag = only_diagnostic(&impossible);
    assert!(
        diag.message.contains("widths"),
        "the failure names the option: {diag:#?}"
    );
}

#[test]
fn img_element_attrs() {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": img
#document("index.html")[
  #img("photo.png", widths: (2, 4), alt: "Photo", sizes: "(max-width: 600px) 480px, 100vw")
]"#,
    );
    png(&site_cfg.config.root().join("photo.png"), [30, 60, 90]);
    let built = cold_build(Arc::new(site_cfg.config.clone()));
    let page = page_text(&built, "index.html");
    assert!(page.contains("src=\"/docs/_tola/images/"), "{page}");
    assert!(page.contains(".webp\""), "{page}");
    assert!(page.contains("srcset=\""), "{page}");
    assert!(page.contains(" 2w") && page.contains(" 4w"), "{page}");
    assert!(
        page.contains("sizes=\"(max-width: 600px) 480px, 100vw\""),
        "{page}"
    );
    assert!(page.contains("width=\"4\""), "{page}");
    assert!(page.contains("height=\"2\""), "{page}");
    assert!(page.contains("alt=\"Photo\""), "{page}");
    assert!(page.contains("loading=\"lazy\""), "{page}");
    assert!(page.contains("decoding=\"async\""), "{page}");
}

#[test]
fn img_source_width_omits_srcset() {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": img
#document("index.html")[#img("photo.png", alt: "Photo")]"#,
    );
    png(&site_cfg.config.root().join("photo.png"), [30, 60, 90]);
    let built = cold_build(Arc::new(site_cfg.config.clone()));
    assert_eq!(derivative_paths(&built).len(), 1);
    let page = page_text(&built, "index.html");
    assert!(page.contains("width=\"8\""), "{page}");
    assert!(page.contains("height=\"4\""), "{page}");
    assert!(!page.contains("srcset="), "{page}");
    assert!(!page.contains("sizes="), "{page}");
}

#[test]
fn refused_requests_report_reason() {
    for (program, expected) in [
        (
            r#"#import "@tola/image:0.0.0": img
#document("index.html")[#img("photo.png", attrs: (src: "photo.png"))]"#,
            "src",
        ),
        (
            r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("logo.svg", width: 4)
#document("index.html")[#html.img(src: resized.url, alt: "Logo")]"#,
            "cannot choose a format for an SVG",
        ),
        (
            r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("linked.svg", width: 4, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: resized.url, alt: "Linked")]"#,
            "inline the image as a data URI",
        ),
        (
            r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("logo.svg")
#document("index.html")[#html.img(src: photo.url, alt: "Logo")]"#,
            "responsive set cannot be derived from an SVG",
        ),
    ] {
        let site_cfg = cfg_with_program(program);
        let root = site_cfg.config.root().to_path_buf();
        png(&root.join("photo.png"), [30, 60, 90]);
        svg(
            &root.join("logo.svg"),
            8,
            4,
            r##"<rect width="8" height="4" fill="#ff0000"/>"##,
        );
        svg(
            &root.join("linked.svg"),
            8,
            4,
            r##"<image href="photo.png"/>"##,
        );
        let diag = only_diagnostic(&site_cfg);
        assert!(
            diag.message.contains(expected),
            "{expected:?} missing from {diag:#?}"
        );
    }
}

#[test]
fn damaged_cache_rebuilds_same_output() {
    let site_cfg = one_image_cfg();
    let cfg = Arc::new(site_cfg.config.clone());
    let storing = cold_build(Arc::clone(&cfg));
    let expected = hashes(&storing);
    damage_stored_variants(cfg.root());

    let rebuilt = cold_build(cfg);
    assert_eq!(hashes(&rebuilt), expected);
}

#[test]
fn failed_render_decoder_reason() {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": resize-image
#let broken = resize-image("broken.png", width: 2, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: broken.url, alt: "Broken")]"#,
    );
    let broken = site_cfg.config.root().join("broken.png");
    png(&broken, [30, 60, 90]);
    strip_pixel_data(&broken);
    let diag = only_diagnostic(&site_cfg);
    assert!(diag.message.contains("broken.png"), "{diag:#?}");
    assert!(
        diag.notes.iter().any(|note| note.contains("damaged")),
        "{diag:#?}"
    );
}

#[test]
fn checks_skip_publish() {
    let site_cfg = cfg_with_program(
        r#"#import "@tola/image:0.0.0": image-metadata, resize-image
#let original = image-metadata("photo.png")
#assert.eq((original.width, original.height, original.format, original.mime), (8, 4, "png", "image/png"))
#let resized = resize-image("photo.png", width: 4, op: "fit-width", format: "webp")
#document("index.html")[#html.img(src: resized.url, alt: "Photo")]"#,
    );
    png(&site_cfg.config.root().join("photo.png"), [50, 100, 150]);
    let cfg = Arc::new(site_cfg.config.clone());
    let mut editor = crate::check::SourceDiagnosticSession::new(Arc::clone(&cfg));
    let checked = editor
        .inspect(Vec::new(), &BuildCancellation::new())
        .unwrap();
    assert!(
        checked.checked().is_some_and(|c| c.bundle().is_some()),
        "{:?}",
        checked.diagnostics()
    );
    assert!(!cfg.build.publish_dir.exists());
    fs::write(
        &cfg.build.entry,
        r#"#import "@tola/image:0.0.0": image-metadata
#let original = image-metadata("photo.png")
#document("index.html")[#original.width x #original.height]"#,
    )
    .unwrap();
    let built = crate::build::build_site(&cfg, BuildMode::Production).unwrap();
    assert!(derivative_outputs(&built).is_empty());
}

struct RebuildCase {
    program: &'static str,
    sources: &'static [(&'static str, [u8; 3])],
    edit: (&'static str, [u8; 3]),
    derivatives: Option<usize>,
}

#[test]
fn retained_build_matches_cold() {
    let cases = [
        RebuildCase {
            program: r#"#import "@tola/image:0.0.0": resize-image
#document("index.html")[
  #context {
    let width = if query(heading).len() == 0 { 2 } else { 4 }
    let resized = resize-image("photo.png", width: width, op: "fit-width", format: "png")
    html.img(src: resized.url, width: resized.width, height: resized.height, alt: "Context")
  }
  = Heading
]"#,
            sources: &[("photo.png", [40, 90, 180])],
            edit: ("photo.png", [180, 90, 40]),
            derivatives: None,
        },
        RebuildCase {
            program: r#"#import "@tola/image:0.0.0": resize-image
#let variants = ("one.png", "two.png").map(source => (
  resize-image(source, width: 4, op: "fit-width", format: "png"),
  resize-image(source, width: 2, op: "fit-width", format: "png"),
))
#document("index.html")[
  #for pair in variants {
    for variant in pair { html.img(src: variant.url, alt: "Variant") }
  }
]"#,
            sources: &[("one.png", [200, 20, 20]), ("two.png", [20, 200, 20])],
            edit: ("two.png", [20, 20, 200]),
            derivatives: Some(4),
        },
        RebuildCase {
            program: r#"#import "@tola/image:0.0.0": resize-image
#let wide = resize-image("one.png", width: 4, op: "fit-width", format: "png")
#let narrow = resize-image("two.png", width: 2, op: "fit-width", format: "png")
#document("index.html")[
  #html.img(src: wide.url, alt: "Wide")
  #html.img(src: narrow.url, alt: "Narrow")
]"#,
            sources: &[("one.png", [200, 20, 20]), ("two.png", [200, 20, 20])],
            edit: ("two.png", [20, 20, 200]),
            derivatives: Some(2),
        },
    ];
    for case in cases {
        let site_cfg = cfg_with_program(case.program);
        let root = site_cfg.config.root().to_path_buf();
        for (name, color) in case.sources {
            png(&root.join(name), *color);
        }
        let cfg = Arc::new(site_cfg.config.clone());
        let mut session = BuildSession::new();
        let first = session
            .prepare(Arc::clone(&cfg), BuildRequest::new(BuildMode::Development))
            .run()
            .unwrap();
        if let Some(derivatives) = case.derivatives {
            assert_eq!(
                derivative_outputs(&first).len(),
                derivatives,
                "{}",
                case.program
            );
        }
        let initial = hashes(&first);
        install(&mut session, first);

        let retained = session
            .prepare(Arc::clone(&cfg), request(Vec::new()))
            .run()
            .unwrap();
        assert_eq!(hashes(&retained), initial, "{}", case.program);
        install(&mut session, retained);

        let (edited, color) = case.edit;
        png(&root.join(edited), color);
        let changed = session
            .prepare(Arc::clone(&cfg), request(vec![root.join(edited)]))
            .run()
            .unwrap();
        assert_eq!(
            hashes(&changed),
            hashes(&cold_build(cfg)),
            "{}",
            case.program
        );
        assert!(!root.join("public").exists());
    }
}
