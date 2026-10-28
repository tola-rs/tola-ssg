//! The site's own files, as the editor names them.
//!
//! Completion offers them, and a site whose program did not compile scans them for the
//! bibliographies its sources name. It is an editor affordance rather than discovery: a file the
//! site never reads still completes. Disk candidates follow the author's ignore files and omit
//! tooling and build state; open sources join completion through the source boundary before saving.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::FileId;

/// The site's own files, as the editor's file ids, in a deterministic order.
pub(super) fn owned_file_ids(
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let root = config.get_root();
    Ok(tola_build::filesystem::owned_files(config, cancellation)?
        .iter()
        .filter_map(|path| tola_typst::file_id_from_path(path, root))
        .collect())
}

pub(super) fn completable_file_ids(
    config: &ResolvedSiteConfig,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
    cancellation: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let root = config.get_root();
    let mut file_ids = owned_file_ids(config, cancellation)?;
    for (path, _) in overrides {
        cancellation.ensure_active()?;
        if let Some(id) = crate::identity::path_id(path, root)
            && boundary.check(path).is_ok()
        {
            file_ids.push(id);
        }
    }
    file_ids.sort_by(|left, right| {
        left.vpath()
            .get_without_slash()
            .cmp(right.vpath().get_without_slash())
    });
    file_ids.dedup();
    Ok(file_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_config(root: &std::path::Path) -> Arc<ResolvedSiteConfig> {
        crate::compiler::tests::site_configuration(root, "")
    }

    fn listed_files(root: &std::path::Path) -> Vec<String> {
        owned_file_ids(&site_config(root), &BuildCancellation::new())
            .unwrap()
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
    fn file_listing_preserves_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let config = site_config(directory.path());
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();

        let error = owned_file_ids(&config, &canceller.token()).unwrap_err();
        assert!(error.is::<tola_build::cancellation::BuildCancelled>());
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
