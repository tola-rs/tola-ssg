//! The site files an author may name in a source.
//!
//! Completion offers them, and a site whose program did not compile scans them for the
//! bibliographies its sources name. It is an editor affordance rather than discovery: a file the
//! site never reads still completes, while the author's own ignore files and Tola's reserved
//! directory keep tooling and build state out of the list.

use ignore::WalkBuilder;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::FileId;

/// Every file below the site root an author may reference, in a deterministic order.
pub(super) fn site_files(config: &ResolvedSiteConfig) -> Vec<FileId> {
    let root = config.get_root();
    let owned_root = root.to_path_buf();
    // Both sides are site-relative: `root_relative` strips the root the walked entries below are
    // stripped into.
    let output = config.root_relative(config.build().publish_dir.clone());
    let mut found: Vec<std::path::PathBuf> = WalkBuilder::new(root)
        // The author's ignore files state what the site does not hold, whether or not the site
        // is a repository of its own.
        .require_git(false)
        .filter_entry(move |entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(&owned_root)
                    .is_ok_and(|relative| relative != output)
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .is_some_and(|kind| kind.is_file() || (kind.is_symlink() && entry.path().is_file()))
        })
        .map(|entry| entry.into_path())
        .collect();
    found.sort();
    found
        .iter()
        .filter_map(|path| tola_typst::file_id_from_path(path, root))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::config::loading::{BuildOverrides, load_site_config};

    fn site_config(root: &std::path::Path) -> ResolvedSiteConfig {
        let configuration = root.join("tola.toml");
        std::fs::write(&configuration, "").expect("a configuration");
        load_site_config(
            Some(&configuration),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .expect("the site configuration loads")
        .into_config()
    }

    fn listed_files(root: &std::path::Path) -> Vec<String> {
        site_files(&site_config(root))
            .iter()
            .map(|id| id.vpath().get_without_slash().to_owned())
            .collect()
    }

    #[test]
    fn site_files_exclude_published_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::write(root.join("content/index.typ"), "").unwrap();
        std::fs::write(root.join("public/index.html"), "published").unwrap();

        assert_eq!(listed_files(root), ["content/index.typ", "tola.toml"]);
    }

    /// A link to a file is a name the author may write; a link to a directory names no file.
    #[test]
    #[cfg(unix)]
    fn site_files_omit_linked_directories() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shared.typ"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("shared.typ"), root.join("linked.typ"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked-directory")).unwrap();

        assert_eq!(listed_files(root), ["linked.typ", "tola.toml"]);
    }
}
