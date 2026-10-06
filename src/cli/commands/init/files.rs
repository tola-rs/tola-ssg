//! Staging of the files and directories `tola init` creates.

use std::path::Path;

use anyhow::Result;
use tola_build::config::section::VendorConfig;

use crate::editor::Editor;
use crate::writes::FileWrites;

use super::config::{self, CONFIG_PATH};
use super::features::Effects;
use super::program;

/// The vendored-package root every scaffold has.
pub(super) const VENDOR_DIR: &str = "vendor";
/// The browser-delivered asset root every scaffold has.
pub(super) const WEB_ASSETS_DIR: &str = "static/web-assets";

/// The directories every scaffold has.
const SITE_DIRECTORIES: &[&str] = &[
    "content",
    VENDOR_DIR,
    "static/web-assets/images",
    "static/web-assets/fonts",
    "static/web-assets/scripts",
    "static/web-assets/css",
    "static/typst-fonts",
    "site",
];

/// Every file and directory `tola init` creates at `root`.
pub(super) fn file_writes(
    root: &Path,
    editors: &[Editor],
    packages: &tola_typst::PackageLocations,
    effects: &Effects,
) -> Result<FileWrites> {
    let schema = config::schema(effects);
    let mut writes = FileWrites::new(root)?;
    for directory in SITE_DIRECTORIES {
        writes.add_directory(directory)?;
    }
    writes.create_file(CONFIG_PATH, config::config_file(&schema))?;
    writes.create_file(
        program::SITE_PATH,
        program::site_program(&effects.outputs()),
    )?;
    writes.create_file(program::SCHEMA_PATH, program::SCHEMA_SOURCE)?;
    writes.create_file(program::PAGE_PATH, program::page_source(&effects.head))?;
    writes.create_file(
        program::NOT_FOUND_PATH,
        program::not_found_source(&effects.head),
    )?;
    writes.create_file(program::SELECTION_PATH, program::SELECTION_SOURCE)?;
    writes.create_file(program::SEO_PATH, program::seo_source(&effects.seo))?;
    for (path, contents) in &effects.files {
        let contents = if *path == "static/tailwind-sources/site.css" {
            contents.replacen(
                "{{stylesheet}}",
                include_str!("templates/stylesheet.css"),
                1,
            )
        } else {
            contents.to_string()
        };
        writes.create_file(path, contents)?;
    }
    let ignore = ignore_file(&schema.build.publish_dir, &schema.vendor, &effects.ignore);
    writes.create_file(".gitignore", ignore.clone())?;
    writes.create_file(".ignore", ignore)?;
    crate::editor::add_initial_files(&mut writes, editors, packages)?;
    Ok(writes)
}

fn ignore_file(output_dir: &Path, vendor: &VendorConfig, feature_lines: &[&str]) -> String {
    let mut content = String::new();
    let mut append_directory = |path: &Path| {
        content.push('/');
        content.extend(
            path.to_string_lossy()
                .trim_matches(['/', '\\'])
                .chars()
                .map(|character| if character == '\\' { '/' } else { character }),
        );
        content.push_str("/\n");
    };
    append_directory(output_dir);
    if let Some(workspace) = tola_build::filesystem::publication_workspace(output_dir) {
        append_directory(&workspace);
    }
    if let Some(workspace) = vendor.workspace_path() {
        append_directory(&workspace);
    }
    for line in feature_lines {
        content.push_str(line);
        content.push('\n');
    }
    content.push_str("/.tola/\n/");
    content.push_str(tola_build::filesystem::SITE_BUILD_LOCK_FILE);
    content.push_str("\n.DS_Store\n");
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands::init::features::{self, Feature, FeatureSet};
    use crate::cli::commands::init::program::{NOT_FOUND_PATH, PAGE_PATH, SCHEMA_PATH};
    use crate::cli::commands::init::selection::selection_issues;

    fn load_generated_site(root: &Path) -> tola_build::config::ResolvedSiteConfig {
        crate::config::load(
            Some(&root.join("tola.toml")),
            tola_build::InputScope::Online,
            tola_typst::PackageLocations::default(),
            &crate::config::ConfigOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    /// The default scaffold `tola init` validates and writes at `root`.
    fn scaffold(root: &Path) {
        let writes =
            super::file_writes(root, &[], &Default::default(), &Effects::default()).unwrap();
        super::super::validate_configuration(&writes, tola_build::InputScope::Online).unwrap();
        writes
            .apply(&tola_build::cancellation::BuildCancellation::default())
            .unwrap();
    }

    /// Every subset of the selectable features.
    fn subsets() -> Vec<FeatureSet> {
        let features = features::selectable_features().collect::<Vec<_>>();
        (0..(1u32 << features.len()))
            .map(|mask| {
                FeatureSet::new(
                    features
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| mask & (1 << *index) != 0)
                        .map(|(_, feature)| *feature),
                )
            })
            .collect()
    }

    /// The scaffold `set` selects, validated and written at `root`.
    fn scaffold_selection(root: &Path, set: &FeatureSet) {
        let effects = features::Effects::combine(set);
        let writes = super::file_writes(root, &[], &Default::default(), &effects).unwrap();
        super::super::validate_configuration(&writes, tola_build::InputScope::Online).unwrap();
        writes
            .apply(&tola_build::cancellation::BuildCancellation::default())
            .unwrap();
    }

    /// The scaffold `set` selects at `root`, with `site.origin` and `site.title` naming the site.
    fn scaffold_selection_with_origin(root: &Path, set: &FeatureSet) {
        scaffold_selection(root, set);
        let configuration = std::fs::read_to_string(root.join("tola.toml")).unwrap();
        std::fs::write(
            root.join("tola.toml"),
            configuration
                .replace("# origin = \"\"", "origin = \"https://example.com\"")
                .replace("title = \"\"", "title = \"Example\""),
        )
        .unwrap();
    }

    /// The scaffold `set` selects at `root`, with the site named and `social-image` set: a card
    /// needs both an origin and a title to be worth declaring.
    fn scaffold_selection_with_social_image(root: &Path, set: &FeatureSet, image: &str) {
        scaffold_selection_with_origin(root, set);
        let configuration = std::fs::read_to_string(root.join("tola.toml")).unwrap();
        std::fs::write(
            root.join("tola.toml"),
            format!("{configuration}\n[site.extra]\nsocial-image = \"{image}\"\n"),
        )
        .unwrap();
    }

    /// The medium scaffold `tola init` validates and writes at `root`.
    fn scaffold_medium(root: &Path) {
        scaffold_selection(root, &features::features("medium"));
    }

    /// The medium scaffold at `root`, with `site.origin` and `site.title` naming the site.
    fn scaffold_medium_with_origin(root: &Path) {
        scaffold_selection_with_origin(root, &features::features("medium"));
    }

    /// Whether `build` publishes `path`.
    fn publishes(build: &tola_build::build::SiteBuild, path: &str) -> bool {
        build
            .graph()
            .outputs()
            .iter()
            .any(|output| output.path().as_str() == path)
    }

    /// The production build of the generated site at `root`.
    fn build_generated_site(root: &Path) -> tola_build::build::SiteBuild {
        let config = load_generated_site(root);
        tola_build::build::build_site(&config, tola_build::build::BuildMode::Production).unwrap()
    }

    /// The published bytes at `path`.
    fn built_output<'a>(build: &'a tola_build::build::SiteBuild, path: &str) -> &'a [u8] {
        build
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == path)
            .expect("the build publishes the requested output")
            .bytes()
    }

    #[test]
    fn not_found_body_reaches_output() {
        let directory = tempfile::tempdir().unwrap();
        scaffold(directory.path());
        std::fs::write(
            directory.path().join(NOT_FOUND_PATH),
            r#"#let not-found-template() = {
  document("404.html", format: "html", title: [Custom])[Custom error page from the site.]
}
"#,
        )
        .unwrap();
        let config = load_generated_site(directory.path());
        assert!(config.warnings().is_empty(), "{:?}", config.warnings());
        let built =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .unwrap();
        assert!(
            built
                .index()
                .address()
                .pages()
                .into_iter()
                .any(|page| page.output.as_str() == "404.html")
        );
        let html = built_output(&built, "404.html");
        assert!(
            std::str::from_utf8(html)
                .unwrap()
                .contains("Custom error page from the site.")
        );
    }

    #[test]
    fn configured_language_reaches_html() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        scaffold(root);
        let configuration = std::fs::read_to_string(root.join("tola.toml")).unwrap();
        std::fs::write(
            root.join("tola.toml"),
            configuration.replace("language = \"en\"", "language = \"zh-CN\""),
        )
        .unwrap();
        std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();
        let build = build_generated_site(root);
        let html = std::str::from_utf8(built_output(&build, "index.html")).unwrap();
        assert!(html.contains("<html lang=\"zh-CN\""), "{html}");
        assert!(html.contains("<main"), "{html}");
    }

    #[test]
    fn source_field_reaches_page_title() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        std::fs::write(
            root.join(SCHEMA_PATH),
            r#"#import "@tola/schema:0.0.0": nullable, optional, schema
#let page-schema = schema((
  heading: content,
  draft: optional(bool, default: false),
  permalink: optional(nullable(str), default: none),
))"#,
        )
        .unwrap();
        std::fs::write(
            root.join(PAGE_PATH),
            r#"#import "@tola/site:0.0.0": site
#import "@tola/web:0.0.0": head-metadata
#let page-template(page) = {
  let source = page.source
  let title = source.meta.heading
  document(page.output, title: title)[
    #html.html[
      #html.head(head-metadata(site, title: title).join())
      #html.body[#include source.file]
    ]
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("content/index.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((heading: [My field]))\nNormal body.",
        )
        .unwrap();
        let built = build_generated_site(root);
        let page = built
            .index()
            .address()
            .pages()
            .into_iter()
            .find(|page| page.output.as_str() == "index.html")
            .unwrap();
        assert_eq!(page.properties.title.as_deref(), Some("My field"));
        let html = built_output(&built, "index.html");
        assert!(
            std::str::from_utf8(html)
                .unwrap()
                .contains("<title>My field</title>")
        );
    }

    #[test]
    fn permalink_drives_published_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        std::fs::write(root.join("content/index.typ"),
            r#"#import "@tola/document:0.0.0": current-document
#import "@tola/source:0.0.0": current-source, tola-meta
#let input = current-source()
#assert.eq(input.path, "index.typ")
#assert.eq(input.filename, "index.typ")
#tola-meta((title: [Document], permalink: "/chosen/document.html", published: datetime(year: 2026, month: 9, day: 1)))
Natural body.
#context {
  assert.eq(current-document().route, "/chosen/document.html")
  [Source: #current-source().path]
}"#
        ).unwrap();
        let built = build_generated_site(root);
        let html = std::str::from_utf8(built_output(&built, "chosen/document.html")).unwrap();
        assert!(html.contains("Natural body"));
        assert!(html.contains("Source: index.typ"), "{html}");
    }

    #[test]
    fn unnamable_segment_blocks_publication() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        let source = root.join("content/CON.typ");
        std::fs::write(
            &source,
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Unpublished], draft: true))
Draft body."#,
        )
        .unwrap();
        let filtered = build_generated_site(root);
        assert_eq!(
            filtered
                .index()
                .address()
                .pages()
                .into_iter()
                .map(|page| page.output.as_str())
                .collect::<Vec<_>>(),
            ["404.html"]
        );

        std::fs::write(
            &source,
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Published], permalink: "/chosen/"))
Published body."#,
        )
        .unwrap();
        let explicit = build_generated_site(root);
        let page = built_output(&explicit, "chosen/index.html");
        assert!(
            std::str::from_utf8(page)
                .unwrap()
                .contains("Published body")
        );

        // Without a permalink the layout segment names the route, and `CON` is a Windows device
        // name: the scaffold reports the address rule instead of publishing the source.
        std::fs::write(
            &source,
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Needs route]))
Selected body."#,
        )
        .unwrap();
        let config = load_generated_site(root);
        let error =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .err()
                .expect("a segment that cannot name a route must fail the build");
        let diagnostics = tola_build::diagnostic::attached(&error).unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("device name")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn drafts_reject_invalid_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        for (name, metadata) in [
            ("bad-title", "draft: true, title: 42"),
            ("bad-tags", "draft: true, tags: (false,)"),
        ] {
            std::fs::write(
                root.join(format!("content/{name}.typ")),
                format!(
                    r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta(({metadata}))
Draft body."#
                ),
            )
            .unwrap();
        }
        let config = load_generated_site(root);
        let error =
            tola_build::build::build_site(&config, tola_build::build::BuildMode::Production)
                .err()
                .expect("draft metadata must be checked before publication is selected");
        let diagnostics = tola_build::diagnostic::attached(&error).unwrap();
        let mut rejected = diagnostics
            .iter()
            .filter_map(|diagnostic| diagnostic.location.as_ref())
            .filter_map(|location| Path::new(&location.path).file_name())
            .map(|name| name.to_str().unwrap())
            .collect::<Vec<_>>();
        rejected.sort_unstable();
        rejected.dedup();
        assert_eq!(rejected, ["bad-tags.typ", "bad-title.typ"]);
    }

    #[test]
    fn retired_declaration_reports_one_warning() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        std::fs::write(
            root.join("content/index.typ"),
            "#metadata((title: [Retired])) <tola-meta>\nRetired body.",
        )
        .unwrap();
        let built = build_generated_site(root);
        let retired = built
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == tola_build::codes::source::DECLARATION_DEPRECATED
            })
            .collect::<Vec<_>>();

        assert_eq!(retired.len(), 1);
        assert!(
            retired[0]
                .notes
                .iter()
                .any(|note| note.contains("content/index.typ")),
            "{:?}",
            retired[0].notes
        );
    }

    #[test]
    fn medium_stylesheet_reaches_rendered_pages() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold_medium(root);
        std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();

        let build = build_generated_site(root);
        for output in ["index.html", "404.html"] {
            let html = std::str::from_utf8(built_output(&build, output)).unwrap();
            assert!(html.contains("href=\"/assets/css/site.css\""), "{html}");
        }
        // The default configuration minifies declared assets, so assert the starter's rules.
        let stylesheet = std::str::from_utf8(built_output(&build, "assets/css/site.css")).unwrap();
        assert!(stylesheet.contains("color-scheme"), "{stylesheet}");
        assert!(stylesheet.contains("body"), "{stylesheet}");
    }

    #[test]
    fn medium_feed_and_sitemap_honor_page_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold_medium_with_origin(root);
        std::fs::write(
            root.join("content/kept.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((published: datetime(year: 2026, month: 9, day: 1), summary: [Kept summary]))\nKept body.",
        )
        .unwrap();
        std::fs::write(
            root.join("content/undated.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Undated]))\nUndated body.",
        )
        .unwrap();
        std::fs::write(
            root.join("content/hidden.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((feed: false, sitemap: false, published: datetime(year: 2026, month: 9, day: 2)))\nHidden body.",
        )
        .unwrap();
        std::fs::write(
            root.join("content/draft.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((draft: true, published: datetime(year: 2026, month: 9, day: 3)))\nDraft body.",
        )
        .unwrap();

        let build = build_generated_site(root);
        let feed = std::str::from_utf8(built_output(&build, "feed.xml")).unwrap();
        assert!(feed.contains("<title>Example</title>"), "{feed}");
        assert!(feed.contains("https://example.com/kept/"), "{feed}");
        assert!(feed.contains("Kept summary"), "{feed}");
        for absent in ["/undated/", "/hidden/", "/draft/"] {
            assert!(!feed.contains(absent), "{feed}");
        }

        let sitemap = std::str::from_utf8(built_output(&build, "sitemap.xml")).unwrap();
        for present in ["https://example.com/kept/", "https://example.com/undated/"] {
            assert!(sitemap.contains(present), "{sitemap}");
        }
        for absent in ["/hidden/", "/draft/"] {
            assert!(!sitemap.contains(absent), "{sitemap}");
        }
    }

    #[test]
    fn absent_origin_omits_feed_and_sitemap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold_medium(root);
        std::fs::write(
            root.join("content/index.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((published: datetime(year: 2026, month: 9, day: 1)))\nBody.",
        )
        .unwrap();

        let build = build_generated_site(root);
        assert!(!publishes(&build, "feed.xml"));
        assert!(!publishes(&build, "sitemap.xml"));
    }

    #[test]
    fn sitemap_uses_origin_without_title() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold_medium(root);
        let configuration = std::fs::read_to_string(root.join("tola.toml")).unwrap();
        std::fs::write(
            root.join("tola.toml"),
            configuration.replace("# origin = \"\"", "origin = \"https://example.com\""),
        )
        .unwrap();
        std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();

        let build = build_generated_site(root);
        let sitemap = std::str::from_utf8(built_output(&build, "sitemap.xml")).unwrap();
        assert!(sitemap.contains("https://example.com/"), "{sitemap}");
        assert!(!publishes(&build, "feed.xml"));
    }

    #[test]
    fn selected_output_publishes_independently() {
        for (feature, present, absent) in [
            (Feature::Feed, "feed.xml", "sitemap.xml"),
            (Feature::Sitemap, "sitemap.xml", "feed.xml"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            scaffold_selection_with_origin(root, &FeatureSet::new([feature]));
            std::fs::write(
                root.join("content/index.typ"),
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((published: datetime(year: 2026, month: 9, day: 1)))\nBody.",
            )
            .unwrap();

            let build = build_generated_site(root);
            assert!(publishes(&build, present), "{feature:?}");
            assert!(!publishes(&build, absent), "{feature:?}");
        }
    }

    #[test]
    fn selected_cards_declare_social_image() {
        for features in [
            vec![Feature::OpenGraph],
            vec![Feature::TwitterCard],
            vec![Feature::OpenGraph, Feature::TwitterCard],
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let selected = FeatureSet::new(features);
            scaffold_selection_with_social_image(root, &selected, "https://example.com/card.png");
            std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();

            let build = build_generated_site(root);
            let html = std::str::from_utf8(built_output(&build, "index.html")).unwrap();
            let not_found = std::str::from_utf8(built_output(&build, "404.html")).unwrap();
            for (feature, attribute) in [
                (Feature::OpenGraph, r#"property="og:image""#),
                (Feature::TwitterCard, r#"name="twitter:image""#),
            ] {
                assert_eq!(
                    html.matches(attribute).count(),
                    usize::from(selected.contains(feature)),
                    "{html}"
                );
                if selected.contains(feature) {
                    assert!(html.contains("https://example.com/card.png"), "{html}");
                }
                assert!(!not_found.contains(attribute), "{not_found}");
            }
        }
    }

    #[test]
    fn absent_social_image_omits_cards() {
        for image in [None, Some("")] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let selected = FeatureSet::new([Feature::OpenGraph, Feature::TwitterCard]);
            match image {
                Some(image) => scaffold_selection_with_social_image(root, &selected, image),
                None => scaffold_selection(root, &selected),
            }
            std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();

            let build = build_generated_site(root);
            let html = std::str::from_utf8(built_output(&build, "index.html")).unwrap();
            assert!(!html.contains("og:image"), "{image:?}: {html}");
            assert!(!html.contains("twitter:image"), "{image:?}: {html}");
        }
    }

    #[test]
    fn selectable_combinations_validate_configuration() {
        for set in subsets() {
            if !selection_issues(&set).is_empty() {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let effects = features::Effects::combine(&set);
            let writes =
                super::file_writes(directory.path(), &[], &Default::default(), &effects).unwrap();
            super::super::validate_configuration(&writes, tola_build::InputScope::Online).unwrap();
        }
    }
}
