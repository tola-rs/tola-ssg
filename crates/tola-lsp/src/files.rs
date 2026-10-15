//! The site's own files, as the editor names them.
//!
//! Completion offers them, and a site whose program did not compile scans them for the
//! bibliographies its sources name. It is an editor affordance rather than discovery: a file the
//! site never reads still completes, while the author's own ignore files and Tola's reserved
//! directory keep tooling and build state out of the list.

use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::FileId;

/// The site's own files, as the editor's file ids, in a deterministic order.
pub(super) fn owned_file_ids(config: &ResolvedSiteConfig) -> Vec<FileId> {
    let root = config.get_root();
    // An editor request holds no build attempt to cancel, and this walk answers one request.
    let cancellation = BuildCancellation::new();
    let paths = match tola_build::filesystem::owned_files(config, &cancellation) {
        Ok(paths) => paths,
        Err(error) => {
            tracing::debug!(%error, "the site's files could not be listed");
            return Vec::new();
        }
    };
    paths
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
        owned_file_ids(&site_config(root))
            .iter()
            .map(|id| id.vpath().get_without_slash().to_owned())
            .collect()
    }

    #[test]
    fn owned_files_exclude_published_output() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::write(root.join("content/index.typ"), "").unwrap();
        std::fs::write(root.join("public/index.html"), "published").unwrap();

        assert_eq!(listed_files(root), ["content/index.typ", "tola.toml"]);
    }

    #[test]
    #[cfg(unix)]
    fn owned_files_omit_linked_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shared.typ"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("shared.typ"), root.join("linked.typ"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked-directory")).unwrap();

        assert_eq!(listed_files(root), ["tola.toml"]);
    }
}
