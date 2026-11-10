//! Staging of the files and directories `tola init` creates.

use std::path::Path;

use anyhow::Result;
use tola_build::config::section::BuildSectionConfig;
use tola_build::filesystem::swap_workspace;

use crate::writes::FileWrites;

use super::config::{self, CONFIG_PATH};

/// The directory a new site starts with; its sources live below it.
const CONTENT_DIR: &str = "content";

/// The program every new site builds from.
const SITE_PATH: &str = "site.typ";
const SITE_SOURCE: &str = include_str!("templates/site.typ");

/// Every file and directory `tola init` creates at `root`.
pub(super) fn file_writes(root: &Path) -> Result<FileWrites> {
    let mut writes = FileWrites::new(root)?;
    writes.add_directory(CONTENT_DIR)?;
    writes.create_file(CONFIG_PATH, config::config_file())?;
    writes.create_file(SITE_PATH, SITE_SOURCE)?;
    let ignore = ignore_file(&BuildSectionConfig::default().publish_dir);
    writes.create_file(".gitignore", ignore.clone())?;
    writes.create_file(".ignore", ignore)?;
    Ok(writes)
}

fn ignore_file(output_dir: &Path) -> String {
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
    if let Some(workspace) = swap_workspace(output_dir) {
        append_directory(&workspace);
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
        let writes = super::file_writes(root).unwrap();
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
    fn initial_site_holds_only_the_minimal_paths() {
        let directory = tempfile::tempdir().unwrap();
        let writes = file_writes(directory.path()).unwrap();
        let mut files = writes
            .file_paths()
            .map(|path| writes.relative_path(path).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files, [".gitignore", ".ignore", "site.typ", "tola.toml"]);
        let directories = writes
            .directory_paths()
            .map(|path| writes.relative_path(path).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(directories, [CONTENT_DIR]);
    }

    #[test]
    fn initial_site_publishes_each_source_and_a_missing_page() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        std::fs::write(root.join("content/note.typ"), "= Note\n").unwrap();
        let build = build_generated_site(root);

        let page = std::str::from_utf8(built_output(&build, "note/index.html")).unwrap();
        assert!(page.contains("<h2>Note</h2>"), "{page}");
        let not_found = std::str::from_utf8(built_output(&build, "404.html")).unwrap();
        assert!(
            not_found.contains("The page you requested could not be found."),
            "{not_found}"
        );
    }

    #[test]
    fn configured_language_reaches_generated_html() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        scaffold(root);
        let configuration = std::fs::read_to_string(root.join("tola.toml")).unwrap();
        std::fs::write(
            root.join("tola.toml"),
            format!("{configuration}\n[site]\nlanguage = \"zh-CN\"\n"),
        )
        .unwrap();
        std::fs::write(root.join("content/index.typ"), "= Welcome\n").unwrap();
        let build = build_generated_site(root);
        let html = std::str::from_utf8(built_output(&build, "index.html")).unwrap();
        assert!(html.contains("<html lang=\"zh-CN\""), "{html}");
    }
}
