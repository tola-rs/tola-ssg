//! Site-owned files exposed to editor completion and bibliography discovery.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tola_build::cancellation::BuildCancellation;
use tola_build::config::SiteConfig;
use tola_typst::typst::syntax::FileId;

/// Site-owned file IDs in deterministic order.
pub(super) fn owned_file_ids(cfg: &SiteConfig, cancel: &BuildCancellation) -> Result<Vec<FileId>> {
    let root = cfg.root();
    Ok(tola_build::filesystem::owned_files(cfg, cancel)?
        .iter()
        .filter_map(|path| tola_typst::file_id_from_path(path, root))
        .collect())
}

pub(super) fn completable_file_ids(
    cfg: &SiteConfig,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
    cancel: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let root = cfg.root();
    let mut ids = owned_file_ids(cfg, cancel)?;
    for (path, _) in overrides {
        cancel.ensure_active()?;
        if let Some(id) = crate::identity::path_id(path, root)
            && boundary.check(path).is_ok()
        {
            ids.push(id);
        }
    }
    ids.sort_by(|a, b| {
        a.vpath()
            .get_without_slash()
            .cmp(b.vpath().get_without_slash())
    });
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_cfg(root: &std::path::Path) -> Arc<SiteConfig> {
        crate::compiler::tests::site_configuration(root, "")
    }

    fn listed_files(root: &std::path::Path) -> Vec<String> {
        owned_file_ids(&site_cfg(root), &BuildCancellation::new())
            .unwrap()
            .iter()
            .map(|id| id.vpath().get_without_slash().to_owned())
            .collect()
    }

    #[cfg(unix)]
    fn symlinks_outside_root_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("shared.typ"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("shared.typ"), root.join("linked.typ"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked-directory")).unwrap();
        assert_eq!(listed_files(root), ["tola.toml"]);
    }

    #[test]
    fn owned_files_exclude_output_and_external_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::write(root.join("content/index.typ"), "").unwrap();
        std::fs::write(root.join("public/index.html"), "published").unwrap();
        assert_eq!(listed_files(root), ["content/index.typ", "tola.toml"]);

        #[cfg(unix)]
        symlinks_outside_root_excluded();
    }

    #[test]
    fn file_listing_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = site_cfg(dir.path());
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();

        let err = owned_file_ids(&cfg, &canceller.token()).unwrap_err();
        assert!(err.is::<tola_build::cancellation::BuildCancelled>());
    }
}
