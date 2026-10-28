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

/// The starter page every scaffold publishes at the site root.
const STARTER_PAGE_PATH: &str = "content/index.typ";
const STARTER_PAGE_SOURCE: &str = include_str!("templates/index-page.typ");

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
    writes.create_file(STARTER_PAGE_PATH, STARTER_PAGE_SOURCE)?;
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
    fn starter_page_publishes_at_the_site_root() {
        let directory = tempfile::tempdir().unwrap();
        scaffold(directory.path());
        let build = build_generated_site(directory.path());
        let html = std::str::from_utf8(built_output(&build, "index.html")).unwrap();

        assert!(html.contains("<title>Welcome</title>"), "{html}");
        assert!(
            html.contains("content=\"A starter page for a site built with Tola\""),
            "{html}"
        );
        assert!(
            html.contains("<h1>Welcome</h1>"),
            "the declared title is the page's first heading: {html}"
        );
        assert!(html.contains("href=\"#start-here\""), "{html}");
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
}
