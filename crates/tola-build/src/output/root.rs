//! Safe creation and replacement of the generated output directory.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::filesystem::{SwapError, SwapMember, directory_is_empty, path_failure_reason};
use tola_diagnostic::display_path;

const PUBLICATION_MARKER_HEADER: &str = "tola-publication\n";
const PUBLICATION_MARKER_FILE: &str = "publication";

/// The record that a published tree is Tola's, inside the reserved namespace it names.
fn publication_marker_path(tree: &Path) -> PathBuf {
    tree.join(tola_address::RESERVED_ROOT)
        .join(PUBLICATION_MARKER_FILE)
}

/// A site-owned output directory that is safe to create or clean.
///
/// The path is strictly below the site root, avoids protected inputs and `.tola`,
/// and traverses no symbolic links. These checks repeat immediately before
/// destructive operations.
#[derive(Debug)]
pub(crate) struct OwnedOutputRoot {
    boundary: crate::filesystem::OutputBoundary,
}

pub(crate) struct OutputWrite<'a> {
    output: &'a OwnedOutputRoot,
    _site_lock: &'a crate::build::SiteBuildLock,
    cancellation: BuildCancellation,
}

#[derive(Debug, Error)]
pub(crate) enum OutputRootError {
    #[error("{0}")]
    Boundary(#[from] crate::filesystem::OutputBoundaryError),

    #[error("{0}")]
    Cancelled(#[from] BuildCancelled),

    #[error("{0}")]
    Swap(#[from] SwapError),

    #[error("the borrowed site lock belongs to a different site")]
    WrongSiteLock,

    #[error(
        "publication workspace `{}` is not owned by this output; move it aside before building",
        display_path(.path, .site_root)
    )]
    UnownedWorkspace { site_root: PathBuf, path: PathBuf },

    #[error(
        "the output directory `{}` already holds files that Tola did not write; move them aside or set `build.publish-dir` to an empty directory",
        display_path(.path, .site_root)
    )]
    Unowned { site_root: PathBuf, path: PathBuf },

    #[error(
        "Tola cannot tell whether it wrote the output directory `{}`; delete that directory or set `build.publish-dir` to another one",
        display_path(.output, .site_root)
    )]
    InvalidMarker { site_root: PathBuf, output: PathBuf },

    #[error("{operation}: {}", path_failure_reason(.source))]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

fn io_error(operation: &'static str, source: io::Error) -> OutputRootError {
    OutputRootError::Io { operation, source }
}

/// Resolve the configured output while protecting inputs discovered by the candidate.
///
/// Producers supply dynamic Bundle reads and external CSS inputs that configuration
/// alone cannot enumerate.
pub(crate) fn resolve_site_output_root_with_inputs<'a>(
    config: &'a crate::config::SiteConfig,
    additional_inputs: impl IntoIterator<Item = (&'static str, &'a Path)>,
) -> anyhow::Result<OwnedOutputRoot> {
    let protected = config
        .protected_input_paths()
        .into_iter()
        .chain(additional_inputs);
    OwnedOutputRoot::resolve(config.root(), &config.build.publish_dir, protected)
        .map_err(Into::into)
}

impl OwnedOutputRoot {
    pub(crate) fn resolve<'a>(
        site_root: &Path,
        output: &Path,
        protected: impl IntoIterator<Item = (&'static str, &'a Path)>,
    ) -> Result<Self, OutputRootError> {
        Ok(Self {
            boundary: crate::filesystem::OutputBoundary::resolve(site_root, output, protected)?,
        })
    }

    #[inline]
    pub(crate) fn path(&self) -> &Path {
        &self.boundary.output
    }

    pub(crate) fn write<'a>(
        &'a self,
        site_lock: &'a crate::build::SiteBuildLock,
        cancellation: &BuildCancellation,
    ) -> Result<OutputWrite<'a>, OutputRootError> {
        cancellation.ensure_active()?;
        if !site_lock.guards(&self.boundary.site_root) {
            return Err(OutputRootError::WrongSiteLock);
        }
        self.revalidate()?;
        self.recover_replacement()?;
        cancellation.ensure_active()?;
        Ok(OutputWrite {
            output: self,
            _site_lock: site_lock,
            cancellation: cancellation.clone(),
        })
    }

    /// Complete or discard the replacement an interrupted build left beside the output.
    ///
    /// A journaled replacement installs its verified tree before this build compiles, so a
    /// later compilation failure cannot leave the site without its last complete output.
    fn recover_replacement(&self) -> Result<(), OutputRootError> {
        let swap = &self.boundary.swap;
        swap.recover().map_err(|error| self.swap_error(error))?;
        swap.finish().map_err(|error| self.swap_error(error))
    }

    /// Report a replacement failure in the terms of the tree publication replaces.
    fn swap_error(&self, error: SwapError) -> OutputRootError {
        match error {
            SwapError::ForeignWorkspace { workspace, .. } => OutputRootError::UnownedWorkspace {
                site_root: self.boundary.site_root.clone(),
                path: workspace,
            },
            error => error.into(),
        }
    }

    /// Refuse a destination Tola cannot prove it wrote; an absent or empty one is replaceable.
    fn ensure_output_is_replaceable(&self) -> Result<(), OutputRootError> {
        match fs::symlink_metadata(&self.boundary.output) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("could not read the output directory", source)),
            Ok(_) => self.boundary.validate_path_chain(&self.boundary.output)?,
        }
        let marker = publication_marker_path(&self.boundary.output);
        self.boundary
            .validate_path_chain(marker.parent().unwrap())?;
        match fs::symlink_metadata(&marker) {
            Ok(_) => self.validate_publication_marker(&self.boundary.output),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if directory_is_empty(&self.boundary.output)
                    .map_err(|source| io_error("could not read the output directory", source))?
                {
                    Ok(())
                } else {
                    Err(OutputRootError::Unowned {
                        site_root: self.boundary.site_root.clone(),
                        path: self.boundary.output.clone(),
                    })
                }
            }
            Err(source) => Err(io_error(
                "could not read Tola's record of the output directory",
                source,
            )),
        }
    }

    fn validate_publication_marker(&self, tree: &Path) -> Result<(), OutputRootError> {
        let marker = publication_marker_path(tree);
        self.boundary
            .validate_path_chain(marker.parent().unwrap())?;
        let mut stored = tola_fs::open_regular_file(&marker, false)
            .map_err(|source| io_error("could not read Tola's output ownership record", source))?;
        let mut contents = Vec::new();
        stored
            .read_to_end(&mut contents)
            .map_err(|source| io_error("could not read Tola's output ownership record", source))?;
        if contents != self.publication_marker_contents() {
            return Err(self.invalid_marker());
        }
        Ok(())
    }

    fn invalid_marker(&self) -> OutputRootError {
        OutputRootError::InvalidMarker {
            site_root: self.boundary.site_root.clone(),
            output: self.boundary.output.clone(),
        }
    }

    fn write_publication_marker(&self, tree: &Path) -> Result<(), OutputRootError> {
        let marker = publication_marker_path(tree);
        let namespace = marker
            .parent()
            .expect("a publication marker names the reserved output directory");
        fs::create_dir_all(namespace).map_err(|source| {
            io_error("could not create Tola's reserved output directory", source)
        })?;
        self.boundary.validate_path_chain(namespace)?;
        crate::filesystem::atomic_write(&marker, &self.publication_marker_contents()).map_err(
            |source| {
                io_error(
                    "could not record that Tola owns the output directory",
                    source,
                )
            },
        )
    }

    // Relative declarations let a site's complete output travel between machines.
    fn publication_marker_contents(&self) -> Vec<u8> {
        let output = self
            .boundary
            .output
            .strip_prefix(&self.boundary.site_root)
            .expect("the output boundary keeps the output below the site root");
        format!(
            "{PUBLICATION_MARKER_HEADER}output={}\n",
            tola_fs::encode_path_identity(output)
        )
        .into_bytes()
    }

    fn revalidate(&self) -> Result<(), OutputRootError> {
        self.boundary.revalidate().map_err(Into::into)
    }
}

impl OutputWrite<'_> {
    pub(crate) fn path(&self) -> &Path {
        &self.output.boundary.output
    }

    /// Claim the replacement workspace and return the tree the candidate is written into.
    pub(crate) fn stage(&self) -> Result<PathBuf, OutputRootError> {
        self.cancellation.ensure_active()?;
        let output = self.output;
        output.revalidate()?;
        let swap = &output.boundary.swap;
        swap.claim().map_err(|error| output.swap_error(error))?;
        let staged = swap.staged(0);
        fs::create_dir_all(&staged)
            .map_err(|source| io_error("could not create the staged output tree", source))?;
        output.write_publication_marker(&staged)?;
        Ok(staged)
    }

    /// Replace the published output with the staged candidate.
    ///
    /// The candidate was verified by the caller before this point; once the replacement is
    /// journaled, a later cleanup failure cannot revoke the published bytes.
    pub(crate) fn publish(self) -> Result<(), OutputRootError> {
        self.cancellation.ensure_active()?;
        let output = self.output;
        output.revalidate()?;
        output.ensure_output_is_replaceable()?;
        let swap = &output.boundary.swap;
        // Recheck cancellation immediately before the replacement starts.
        self.cancellation.ensure_active()?;
        swap.commit(&[SwapMember {
            staged: swap.staged(0),
            destination: output.boundary.output.clone(),
        }])
        .map_err(|error| output.swap_error(error))?;
        // The installed tree is committed; cleanup cannot revoke success.
        if let Err(error) = swap.finish() {
            tracing::warn!(
                error = %error,
                output = %output.boundary.output.display(),
                "published output; deferred cleanup of the previous tree"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfig;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn root_alias_canonical_path() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("site");
        fs::create_dir(&root).unwrap();
        let canon = fs::canonicalize(&root).unwrap();
        let alias = dir.path().join("alias");
        symlink(&root, &alias).unwrap();

        for site_root in [&canon, &alias] {
            let owned = OwnedOutputRoot::resolve(site_root, &alias.join("public"), []).unwrap();
            assert_eq!(owned.path(), canon.join("public"));
        }
    }

    fn site_lock(out: &OwnedOutputRoot) -> crate::build::SiteBuildLock {
        crate::output::tests::site_lock(&out.boundary.site_root)
    }

    fn publish(out: &OwnedOutputRoot, files: &[(&str, &str)]) -> Result<(), OutputRootError> {
        let lock = site_lock(out);
        let writer = out.write(&lock, &BuildCancellation::default())?;
        let staged = writer.stage()?;
        for (name, contents) in files {
            let path = staged.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        writer.publish()
    }

    fn published_output(site_root: &Path, files: &[(&str, &str)]) -> OwnedOutputRoot {
        let out = OwnedOutputRoot::resolve(site_root, &site_root.join("public"), []).unwrap();
        publish(&out, files).unwrap();
        out
    }

    fn resolve_configured_output(cfg: &SiteConfig) -> anyhow::Result<OwnedOutputRoot> {
        resolve_site_output_root_with_inputs(cfg, std::iter::empty())
    }

    fn output_test_config(root: &Path) -> SiteConfig {
        let root = root.canonicalize().unwrap();
        let mut cfg = crate::config::tests::load_test_config(&root, "");
        cfg.config_path = root.join("tola.toml");
        cfg.build.publish_dir = root.join("public");
        cfg.build.entry = root.join("site.typ");
        cfg.build.content_dir = root.join("content");
        cfg
    }

    fn assert_protected_input(label: &str, err: anyhow::Error, kind: &str) {
        assert!(
            matches!(
                err.downcast_ref::<OutputRootError>(),
                Some(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::ProtectedPath { kind: found, .. }
                )) if *found == kind
            ),
            "{label}: {err:#}"
        );
    }

    #[test]
    fn output_rejects_protected_inputs() {
        use crate::config::section::IconCollectionSource;
        let dir = TempDir::new().unwrap();

        for src in [
            IconCollectionSource::LocalJson {
                path: dir.path().join("public/icons.json"),
            },
            IconCollectionSource::LocalSvgDir {
                path: dir.path().join("public/icons"),
            },
        ] {
            let mut cfg = output_test_config(dir.path());
            cfg.icons.collections.insert("brand".into(), src);
            let err = resolve_configured_output(&cfg).unwrap_err();
            assert_protected_input("icon source", err, "icon source");
        }

        let mut cfg = output_test_config(dir.path());
        cfg.config_path = cfg.build.publish_dir.join("tola.toml");
        assert_protected_input(
            "config",
            resolve_configured_output(&cfg).unwrap_err(),
            "config",
        );

        let mut cfg = output_test_config(dir.path());
        cfg.build.entry = cfg.build.publish_dir.join("entry.typ");
        assert_protected_input(
            "build entry",
            resolve_configured_output(&cfg).unwrap_err(),
            "build entry",
        );

        let mut cfg = output_test_config(dir.path());
        cfg.build.content_dir = cfg.build.publish_dir.join("content");
        assert_protected_input(
            "content-dir",
            resolve_configured_output(&cfg).unwrap_err(),
            "content-dir",
        );

        for generated in ["public", ".public-swap"] {
            let cfg = output_test_config(dir.path());
            let read = cfg.root().join(generated).join("templates/document.typ");
            let err = resolve_site_output_root_with_inputs(&cfg, [("Typst read", read.as_path())])
                .unwrap_err();
            assert_protected_input("Typst read", err, "Typst read");
        }

        for (rel, author_file) in [
            (Path::new(".tola"), None),
            (Path::new(".tola/nested"), None),
            (Path::new(".tola/public"), Some("author file")),
        ] {
            let mut cfg = output_test_config(dir.path());
            cfg.build.publish_dir = cfg.root().join(rel);
            if let Some(author_file) = author_file {
                let internal = cfg.root().join(tola_input::INTERNAL_DIR);
                fs::create_dir_all(&internal).unwrap();
                fs::write(&cfg.build.publish_dir, author_file).unwrap();
            }
            let err = resolve_configured_output(&cfg).unwrap_err();
            assert_protected_input("`.tola` directory", err, "`.tola` directory");
            if let Some(author_file) = author_file {
                assert_eq!(
                    fs::read_to_string(&cfg.build.publish_dir).unwrap(),
                    author_file
                );
            }
        }

        let out = dir.path().join("public");
        for (pkg_path, pkg_cache, kind) in [
            (Some(out.join("packages")), None, "Typst package"),
            (None, Some(out.join("cache")), "Typst package cache"),
        ] {
            let mut cfg = output_test_config(dir.path());
            cfg.package_locations = tola_typst::PackageLocations::discover(
                pkg_path.or_else(|| Some(dir.path().join("safe-data"))),
                pkg_cache.or_else(|| Some(dir.path().join("safe-cache"))),
            )
            .unwrap();
            let err = resolve_configured_output(&cfg).unwrap_err();
            assert_protected_input("package root", err, kind);
        }
    }

    #[cfg(windows)]
    #[test]
    fn output_rejects_case_aliased_internal() {
        let dir = TempDir::new().unwrap();
        let mut cfg = output_test_config(dir.path());
        fs::create_dir(cfg.root().join(tola_input::INTERNAL_DIR)).unwrap();
        cfg.build.publish_dir = cfg.root().join(".TOLA/output");
        assert_protected_input(
            "case-aliased `.tola`",
            resolve_configured_output(&cfg).unwrap_err(),
            "`.tola` directory",
        );
    }

    #[test]
    fn output_overlap_real_components() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("site");
        fs::create_dir(&root).unwrap();

        for out in [
            root.clone(),
            dir.path().to_path_buf(),
            dir.path().join("other"),
        ] {
            assert!(matches!(
                OwnedOutputRoot::resolve(&root, &out, []),
                Err(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::OutsideSite
                ))
            ));
        }

        for (out, protected, kind) in [
            (
                dir.path().join("public"),
                dir.path().join("public"),
                "content",
            ),
            (
                dir.path().join("content/public"),
                dir.path().join("content"),
                "generated input",
            ),
        ] {
            assert!(matches!(
                OwnedOutputRoot::resolve(dir.path(), &out, [(kind, protected.as_path())]),
                Err(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::ProtectedPath { kind: found, .. }
                )) if found == kind
            ));
        }

        for (out, protected, expected) in [
            ("public", "publicx", "public"),
            ("a", "ab", "a"),
            ("site", "site-assets", "site"),
            ("output", "outputs", "output"),
            ("site/public", "site/content", "site/public"),
        ] {
            let owned = OwnedOutputRoot::resolve(
                dir.path(),
                &dir.path().join(out),
                [("generated input", dir.path().join(protected).as_path())],
            )
            .unwrap_or_else(|err| panic!("{out} overlaps {protected}: {err}"));
            assert_eq!(
                owned.path(),
                dir.path().canonicalize().unwrap().join(expected)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_symlinks_rejected() {
        use std::os::unix::fs::symlink;

        {
            let dir = TempDir::new().unwrap();
            let root = dir.path().join("site");
            let ext = dir.path().join("external");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&ext).unwrap();
            symlink(&ext, root.join("public")).unwrap();
            let expected = root.canonicalize().unwrap().join("public");
            assert!(matches!(
                OwnedOutputRoot::resolve(&root, &root.join("public/site"), []),
                Err(OutputRootError::Boundary(crate::filesystem::OutputBoundaryError::Symlink { path, .. })) if path == expected
            ));
        }

        {
            let dir = TempDir::new().unwrap();
            let out = dir.path().join("public");
            let ext = dir.path().join("external");
            fs::create_dir(&ext).unwrap();
            fs::write(ext.join("keep.txt"), "external data").unwrap();
            let owned = OwnedOutputRoot::resolve(dir.path(), &out, []).unwrap();
            let expected = owned.path().to_path_buf();
            symlink(&ext, &out).unwrap();
            let lock = site_lock(&owned);
            assert!(matches!(
                owned.write(&lock, &BuildCancellation::default()),
                Err(OutputRootError::Boundary(crate::filesystem::OutputBoundaryError::Symlink { path, .. })) if path == expected
            ));
            assert_eq!(
                fs::read_to_string(ext.join("keep.txt")).unwrap(),
                "external data"
            );
        }

        {
            let dir = TempDir::new().unwrap();
            let out = dir.path().join("public");
            let protected = dir.path().join("assets");
            fs::create_dir(&out).unwrap();
            symlink(&out, &protected).unwrap();
            assert!(matches!(
                OwnedOutputRoot::resolve(dir.path(), &out, [("asset", protected.as_path())]),
                Err(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::ProtectedPath { kind: "asset", .. }
                ))
            ));
        }
    }

    #[test]
    fn foreign_ownership_rejected() {
        for marker in [None, Some("not an owner marker")] {
            let dir = TempDir::new().unwrap();
            let out = dir.path().join("public");
            fs::create_dir(&out).unwrap();
            fs::write(out.join("keep.txt"), "user data").unwrap();
            let owned = OwnedOutputRoot::resolve(dir.path(), &out, []).unwrap();
            if let Some(marker) = marker {
                let path = publication_marker_path(owned.path());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, marker).unwrap();
            }
            assert!(publish(&owned, &[]).is_err(), "{marker:?}");
            assert_eq!(
                fs::read_to_string(out.join("keep.txt")).unwrap(),
                "user data"
            );
        }

        for marker in [None, Some("another owner")] {
            let dir = TempDir::new().unwrap();
            let out = dir.path().join("public");
            let workspace = crate::filesystem::swap_workspace(&out).unwrap();
            fs::create_dir_all(workspace.join("previous")).unwrap();
            fs::write(workspace.join("previous/keep.txt"), "author files").unwrap();
            if let Some(marker) = marker {
                fs::write(workspace.join("owner"), marker).unwrap();
            }
            let owned = OwnedOutputRoot::resolve(dir.path(), &out, []).unwrap();
            let lock = site_lock(&owned);
            assert!(matches!(
                owned.write(&lock, &BuildCancellation::default()),
                Err(OutputRootError::UnownedWorkspace { .. })
            ));
            assert_eq!(
                fs::read(workspace.join("previous/keep.txt")).unwrap(),
                b"author files"
            );
            assert!(!out.exists());
        }

        {
            let dir = TempDir::new().unwrap();
            let out_a = dir.path().join("public-a");
            let out_b = dir.path().join("public-b");
            let owned_a = OwnedOutputRoot::resolve(dir.path(), &out_a, []).unwrap();
            publish(&owned_a, &[("index.html", "a")]).unwrap();

            fs::create_dir(&out_b).unwrap();
            fs::write(out_b.join("keep.txt"), "user data").unwrap();
            let owned_b = OwnedOutputRoot::resolve(dir.path(), &out_b, []).unwrap();
            let marker = publication_marker_path(&out_b);
            fs::create_dir_all(marker.parent().unwrap()).unwrap();
            fs::copy(publication_marker_path(&out_a), marker).unwrap();

            let lock = site_lock(&owned_b);
            let writer = owned_b.write(&lock, &BuildCancellation::default()).unwrap();
            assert!(matches!(
                writer.publish(),
                Err(OutputRootError::InvalidMarker { .. })
            ));
            assert_eq!(
                fs::read_to_string(out_b.join("keep.txt")).unwrap(),
                "user data"
            );
        }

        {
            let dir = TempDir::new().unwrap();
            let other = TempDir::new().unwrap();
            let out = OwnedOutputRoot::resolve(dir.path(), &dir.path().join("public"), []).unwrap();
            let foreign =
                OwnedOutputRoot::resolve(other.path(), &other.path().join("public"), []).unwrap();
            let lock = site_lock(&foreign);
            assert!(matches!(
                out.write(&lock, &BuildCancellation::default()),
                Err(OutputRootError::WrongSiteLock)
            ));
            assert!(!out.path().exists());
        }
    }

    #[test]
    fn publication_clean_tree() {
        let dir = TempDir::new().unwrap();
        let owned = published_output(dir.path(), &[("stale.html", "stale")]);
        publish(&owned, &[("index.html", "home")]).unwrap();

        assert!(!owned.path().join("stale.html").exists());
        assert!(
            !crate::filesystem::swap_workspace(owned.path())
                .unwrap()
                .exists()
        );
        assert_eq!(
            fs::read_to_string(owned.path().join("index.html")).unwrap(),
            "home"
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_rejects_aliases() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("public");
        let workspace = crate::filesystem::swap_workspace(&out).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("page.typ"), "source").unwrap();
        let alias = dir.path().join("page.typ");
        std::os::unix::fs::symlink(workspace.join("page.typ"), &alias).unwrap();

        assert!(matches!(
            OwnedOutputRoot::resolve(dir.path(), &out, [("Typst read", alias.as_path())]),
            Err(OutputRootError::Boundary(
                crate::filesystem::OutputBoundaryError::ProtectedPath {
                    kind: "Typst read",
                    ..
                }
            ))
        ));
        assert_eq!(fs::read(&alias).unwrap(), b"source");
    }

    #[cfg(unix)]
    #[test]
    fn published_survives_cleanup_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let owned = published_output(dir.path(), &[("index.html", "previous")]);
        let locked = owned.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("keep.txt"), "keep").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        publish(&owned, &[("index.html", "committed")]).unwrap();

        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"committed"
        );
        let prev = crate::filesystem::swap_workspace(owned.path())
            .unwrap()
            .join("previous/0/locked");
        if prev.exists() {
            fs::set_permissions(prev, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn cancelled_publish_noop() {
        let dir = TempDir::new().unwrap();
        let owned = published_output(dir.path(), &[("index.html", "successful")]);
        let canceller = crate::cancellation::BuildCanceller::default();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &canceller.token()).unwrap();
        let staged = writer.stage().unwrap();
        fs::write(staged.join("index.html"), "candidate").unwrap();
        canceller.cancel();

        let err = writer.publish().unwrap_err();
        assert!(crate::cancellation::is_cancelled(&err.into()));
        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"successful"
        );
    }
}
