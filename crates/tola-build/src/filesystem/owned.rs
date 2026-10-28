//! The files the site owns below its root.

use std::path::{Path, PathBuf};

use anyhow::Result;
use ignore::WalkBuilder;

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;

use super::{INTERNAL_DIR, display_path, path_failure_reason};

/// Every file below the site root that the site owns, absolute and sorted.
///
/// The author's `.gitignore` and `.ignore` files decide what the site holds. A dot-named entry, a
/// symbolic link, and the state Tola or a tool beside the site keeps — [`INTERNAL_DIR`], the
/// publish directory, a declared `[vendor] path`, `target`, `node_modules` — are not the site's
/// own files.
pub fn owned_files(
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<Vec<PathBuf>> {
    let root = config.get_root();
    // The entry filter outlives this call, so the two declared directories travel as owned values.
    let publish_dir = config.build().publish_dir.clone();
    let vendor_path = config.vendor().path.clone();
    let mut files = Vec::new();
    for entry in WalkBuilder::new(root)
        // A site need not be a repository of its own, and its ignore files apply either way.
        .require_git(false)
        .filter_entry(move |entry| !holds_state(entry, &publish_dir, vendor_path.as_deref()))
        .build()
    {
        cancellation.ensure_active()?;
        let entry = entry.map_err(|error| failed(root, &error))?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

/// Whether the walk must not enter `entry`: state rather than the site's own files.
fn holds_state(entry: &ignore::DirEntry, publish_dir: &Path, vendor_path: Option<&Path>) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    if matches!(
        entry.file_name().to_str(),
        Some("target" | "node_modules" | INTERNAL_DIR)
    ) {
        return true;
    }
    let path = entry.path();
    path == publish_dir || vendor_path == Some(path)
}

/// The failure the walk stopped on, named as the author reads it: the site path it happened at,
/// with the reason the filesystem gave when it gave one.
fn failed(root: &Path, error: &ignore::Error) -> anyhow::Error {
    let subject = match failed_path(error) {
        Some(path) => format!("`{}`", display_path(path, root)),
        None => "the site's files".to_owned(),
    };
    match error.io_error().map(path_failure_reason) {
        Some(reason) => anyhow::anyhow!("cannot read {subject}: {reason}"),
        None => anyhow::anyhow!("cannot read {subject}"),
    }
}

/// The site path the walker tagged a failure with, when it tagged one.
fn failed_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            failed_path(err)
        }
        ignore::Error::Partial(errors) => errors.iter().find_map(failed_path),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{OwnedSiteConfig, VENDORED_SITE_SOURCE};

    /// The site's own files as the author reads them.
    fn listed_files(site: &OwnedSiteConfig) -> Vec<String> {
        owned_files(&site.config, &BuildCancellation::new())
            .expect("the site's files are walkable")
            .iter()
            .map(|path| display_path(path, site.config.get_root()))
            .collect()
    }

    /// An empty file at `relative` below `root`, with the directories it needs.
    fn write_file(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    /// A site owns what an author reads: reserved build directories, a declared vendor tree,
    /// ignored subtrees, and hidden entries never join the walk. Each row owns its own site, so
    /// one exclusion cannot mask another.
    #[test]
    fn owned_files_exclude_non_author_paths() {
        for excluded in [
            "target/page.typ",
            "node_modules/page.typ",
            "public/page.typ",
            &format!("{INTERNAL_DIR}/page.typ"),
            ".draft.typ",
            ".cache/value.typ",
        ] {
            let site = OwnedSiteConfig::new("");
            write_file(site.config.get_root(), excluded);
            assert_eq!(listed_files(&site), ["tola.toml"], "walked `{excluded}`");
        }

        let vendored = OwnedSiteConfig::new(VENDORED_SITE_SOURCE);
        write_file(
            vendored.config.get_root(),
            "vendor/typst-packages/preview/demo/1.0.0/lib.typ",
        );
        assert_eq!(listed_files(&vendored), ["tola.toml"], "walked vendor tree");

        let ignored = OwnedSiteConfig::new("");
        let root = ignored.config.get_root();
        write_file(root, "drafts/page.typ");
        write_file(root, "scratch.typ");
        std::fs::write(root.join(".gitignore"), "drafts/\n").unwrap();
        std::fs::write(root.join(".ignore"), "scratch.typ\n").unwrap();
        assert_eq!(listed_files(&ignored), ["tola.toml"], "walked ignored path");
    }

    #[test]
    #[cfg(unix)]
    fn symbolic_links_are_not_owned_files() {
        let site = OwnedSiteConfig::new("");
        let root = site.config.get_root();
        let outside = tempfile::tempdir().unwrap();
        write_file(outside.path(), "shared.typ");
        write_file(root, "content/index.typ");
        std::os::unix::fs::symlink(outside.path().join("shared.typ"), root.join("linked.typ"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linked-directory")).unwrap();

        assert_eq!(listed_files(&site), ["content/index.typ", "tola.toml"]);
    }

    #[test]
    fn listed_paths_are_absolute_and_sorted() {
        let site = OwnedSiteConfig::new("");
        let root = site.config.get_root();
        for path in [
            "z.typ",
            "content/nested/deep.typ",
            "content/index.typ",
            "a.typ",
        ] {
            write_file(root, path);
        }

        let files = owned_files(&site.config, &BuildCancellation::new()).unwrap();
        assert!(
            files
                .iter()
                .all(|path| path.is_absolute() && path.starts_with(root))
        );
        assert_eq!(
            listed_files(&site),
            [
                "a.typ",
                "content/index.typ",
                "content/nested/deep.typ",
                "tola.toml",
                "z.typ"
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_directory_names_its_site_path() {
        use std::os::unix::fs::PermissionsExt;

        let site = OwnedSiteConfig::new("");
        let root = site.config.get_root();
        write_file(root, "content/closed/page.typ");
        let closed = root.join("content/closed");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        // A privileged user reads the directory whatever its mode, so the refusal under test
        // would not happen.
        if std::fs::read_dir(&closed).is_ok() {
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }

        let error = owned_files(&site.config, &BuildCancellation::new())
            .expect_err("an unreadable directory fails the walk");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();

        let message = format!("{error:#}");
        assert!(message.contains("content/closed"), "{message}");
        assert!(message.contains("permission was denied"), "{message}");
        assert!(
            !message.contains(root.to_string_lossy().as_ref()),
            "{message}"
        );
    }

    #[test]
    fn cancelled_walk_stops() {
        let site = OwnedSiteConfig::new("");
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = owned_files(&site.config, &cancellation).expect_err("a cancelled walk");
        assert!(crate::cancellation::is_cancelled(&error));
    }
}
