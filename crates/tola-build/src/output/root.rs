//! Safe creation and replacement of the generated output directory.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::filesystem::{display_path, encode_path_identity, path_failure_reason};

const OWNER_MARKER_HEADER: &str = "tola-output-root\n";
const OWNER_MARKER_FILE: &str = "owner";
const WORKSPACE_MARKER_HEADER: &str = "tola-publication-workspace\n";

/// The record that a published tree is Tola's, inside the reserved namespace it names.
fn owner_marker_path(tree: &Path) -> PathBuf {
    tree.join(tola_address::RESERVED_ROOT)
        .join(OWNER_MARKER_FILE)
}
const PREVIOUS_OUTPUT_DIRECTORY: &str = "previous";

/// Prefix of the private tree a staged build writes below the publication workspace.
pub(super) const STAGING_DIRECTORY_PREFIX: &str = "staging-";

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

enum ExistingOutput {
    Absent,
    Empty,
    Owned,
}

#[derive(Debug, Error)]
pub(crate) enum OutputRootError {
    #[error("{0}")]
    Boundary(#[from] crate::filesystem::OutputBoundaryError),

    #[error("{0}")]
    Cancelled(#[from] BuildCancelled),

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
    config: &'a crate::config::ResolvedSiteConfig,
    additional_inputs: impl IntoIterator<Item = (&'static str, &'a Path)>,
) -> anyhow::Result<OwnedOutputRoot> {
    let protected = config
        .protected_input_paths()
        .into_iter()
        .chain(additional_inputs);
    OwnedOutputRoot::resolve(config.get_root(), &config.build.publish_dir, protected)
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
        let writing = OutputWrite {
            output: self,
            _site_lock: site_lock,
            cancellation: cancellation.clone(),
        };
        writing.recover_previous()?;
        cancellation.ensure_active()?;
        Ok(writing)
    }

    /// The publication workspace, after recording this site's ownership of it.
    pub(crate) fn output_workspace(&self) -> Result<&Path, OutputRootError> {
        self.revalidate()?;
        self.adopt_workspace()?;
        Ok(&self.boundary.workspace)
    }

    /// Record this site's ownership of an existing workspace.
    ///
    /// A build killed between creating the workspace and recording its owner leaves an empty
    /// directory behind, and nothing else can have written it, so this site records ownership
    /// of that directory where it stands. A workspace holding any entry stays foreign: Tola
    /// never deletes a tree it did not write.
    fn adopt_workspace(&self) -> Result<(), OutputRootError> {
        let workspace = &self.boundary.workspace;
        match fs::symlink_metadata(workspace) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(io_error("could not read the publication workspace", source));
            }
            Ok(_) => self.boundary.validate_path_chain(workspace)?,
        }
        let marker = workspace.join(OWNER_MARKER_FILE);
        if marker_matches(&marker, &self.marker_contents(WORKSPACE_MARKER_HEADER)).unwrap_or(false)
        {
            return Ok(());
        }
        if !directory_is_empty(workspace)
            .map_err(|source| io_error("could not read the publication workspace", source))?
        {
            return Err(OutputRootError::UnownedWorkspace {
                site_root: self.boundary.site_root.clone(),
                path: workspace.clone(),
            });
        }
        crate::filesystem::atomic_write(&marker, &self.marker_contents(WORKSPACE_MARKER_HEADER))
            .map_err(|source| io_error("could not record publication workspace ownership", source))
    }

    fn existing_output(&self) -> Result<ExistingOutput, OutputRootError> {
        match fs::symlink_metadata(&self.boundary.output) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ExistingOutput::Absent);
            }
            Err(source) => return Err(io_error("could not read the output directory", source)),
            Ok(_) => self.boundary.validate_path_chain(&self.boundary.output)?,
        }
        let marker = owner_marker_path(&self.boundary.output);
        self.boundary
            .validate_path_chain(marker.parent().unwrap())?;
        match fs::symlink_metadata(&marker) {
            Ok(_) => {
                self.validate_owner_marker(&self.boundary.output)?;
                Ok(ExistingOutput::Owned)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if directory_is_empty(&self.boundary.output)
                    .map_err(|source| io_error("could not read the output directory", source))?
                {
                    Ok(ExistingOutput::Empty)
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

    fn validate_owner_marker(&self, tree: &Path) -> Result<(), OutputRootError> {
        let marker = owner_marker_path(tree);
        self.boundary
            .validate_path_chain(marker.parent().unwrap())?;
        if !marker_matches(&marker, &self.marker_contents(OWNER_MARKER_HEADER))
            .map_err(|source| io_error("could not read Tola's output ownership record", source))?
        {
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

    fn write_owner_marker(&self, tree: &Path) -> Result<(), OutputRootError> {
        let marker = owner_marker_path(tree);
        let namespace = marker
            .parent()
            .expect("an owner marker names the reserved output directory");
        fs::create_dir_all(namespace).map_err(|source| {
            io_error("could not create Tola's reserved output directory", source)
        })?;
        self.boundary.validate_path_chain(namespace)?;
        let contents = self.marker_contents(OWNER_MARKER_HEADER);
        crate::filesystem::atomic_write(&marker, &contents).map_err(|source| {
            io_error(
                "could not record that Tola owns the output directory",
                source,
            )
        })
    }

    // Relative declarations let a site's complete output travel between machines.
    fn marker_contents(&self, header: &str) -> Vec<u8> {
        let output = self
            .boundary
            .output
            .strip_prefix(&self.boundary.site_root)
            .expect("the output boundary keeps the output below the site root");
        format!("{header}output={}\n", encode_path_identity(output)).into_bytes()
    }

    fn revalidate(&self) -> Result<(), OutputRootError> {
        self.boundary.revalidate().map_err(Into::into)
    }
}

impl OutputWrite<'_> {
    pub(crate) fn path(&self) -> &Path {
        &self.output.boundary.output
    }

    /// Restore the verified previous tree when it is the only complete output.
    ///
    /// A destination that holds nothing is not a competing tree: an empty directory is removed
    /// and the previous output takes its place, exactly as a publish replaces one. Only
    /// `remove_dir` is used, so a destination holding anything else is never deleted.
    fn recover_previous(&self) -> Result<(), OutputRootError> {
        let output = self.output;
        let previous = output.output_workspace()?.join(PREVIOUS_OUTPUT_DIRECTORY);
        match fs::symlink_metadata(&previous) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("could not read the previous site output", source)),
            Ok(_) => output.boundary.validate_path_chain(&previous)?,
        }
        output.validate_owner_marker(&previous)?;
        self.cancellation.ensure_active()?;
        let existing = output.existing_output()?;
        // Once the previous tree is the only complete one, finish restoring it even when
        // cancellation arrives; the borrowed site lock covers the rest of this operation.
        match existing {
            ExistingOutput::Owned => self.remove_previous(&previous)?,
            ExistingOutput::Absent | ExistingOutput::Empty => {
                if matches!(existing, ExistingOutput::Empty) {
                    fs::remove_dir(&output.boundary.output).map_err(|source| {
                        io_error("could not remove the empty output directory", source)
                    })?;
                }
                fs::rename(&previous, &output.boundary.output).map_err(|source| {
                    io_error("could not restore the previous site output", source)
                })?;
            }
        }
        Ok(())
    }

    fn remove_previous(&self, previous: &Path) -> Result<(), OutputRootError> {
        self.output.output_workspace()?;
        self.output.boundary.validate_path_chain(previous)?;
        self.output.validate_owner_marker(previous)?;
        fs::remove_dir_all(previous)
            .map_err(|source| io_error("could not clean up the previous site output", source))
    }

    pub(crate) fn prepare(&self) -> Result<(), OutputRootError> {
        self.cancellation.ensure_active()?;
        self.output.revalidate()?;
        self.output.existing_output()?;
        Ok(())
    }

    /// Create the publication workspace when it is absent, and record this site's ownership.
    fn prepare_workspace(&self) -> Result<&Path, OutputRootError> {
        self.cancellation.ensure_active()?;
        let output = self.output;
        output.revalidate()?;
        let workspace = &output.boundary.workspace;
        fs::create_dir_all(workspace.parent().unwrap())
            .map_err(|source| io_error("could not create the output parent directory", source))?;
        output.revalidate()?;
        if let Err(source) = fs::create_dir(workspace)
            && source.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(io_error(
                "could not create the publication workspace",
                source,
            ));
        }
        output.output_workspace()
    }

    /// Prepare the workspace for a new staged tree, reclaiming the trees a killed build left.
    pub(crate) fn prepare_workspace_for_staging(&self) -> Result<&Path, OutputRootError> {
        let workspace = self.prepare_workspace()?;
        self.reclaim_abandoned_staging()?;
        Ok(workspace)
    }

    /// Remove the staging trees a killed build left in the publication workspace.
    ///
    /// The site lock is held and the workspace record names this output, so no running build
    /// owns them: each holds only a candidate that was never committed. Only a directory whose
    /// name has the staging prefix is removed; every other entry stays untouched.
    fn reclaim_abandoned_staging(&self) -> Result<(), OutputRootError> {
        let workspace = &self.output.boundary.workspace;
        let entries = fs::read_dir(workspace)
            .map_err(|source| io_error("could not read the publication workspace", source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| io_error("could not read the publication workspace", source))?;
            let name = entry.file_name();
            if !name
                .to_str()
                .is_some_and(|name| name.starts_with(STAGING_DIRECTORY_PREFIX))
            {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|source| io_error("could not read the abandoned staging tree", source))?;
            if !file_type.is_dir() {
                continue;
            }
            self.cancellation.ensure_active()?;
            fs::remove_dir_all(entry.path()).map_err(|source| {
                io_error("could not clean up the abandoned staging tree", source)
            })?;
        }
        Ok(())
    }

    /// Readers can observe the output briefly absent between ordinary directory
    /// renames, but never a partially materialized graph.
    pub(crate) fn replace_with(self, staging: tempfile::TempDir) -> Result<(), OutputRootError> {
        self.replace_with_rename(staging, |from, to| fs::rename(from, to))
    }

    fn replace_with_rename(
        self,
        staging: tempfile::TempDir,
        mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    ) -> Result<(), OutputRootError> {
        self.prepare()?;
        let output = self.output;
        let staging_path = crate::filesystem::site_relative_absolute(
            &output.boundary.site_root,
            &output.boundary.site_root,
            staging.path(),
        )
        .map_err(crate::filesystem::OutputBoundaryError::from)?;
        output.boundary.validate_path_chain(&staging_path)?;
        output.write_owner_marker(&staging_path)?;
        let previous = self.prepare_workspace()?.join(PREVIOUS_OUTPUT_DIRECTORY);
        match fs::symlink_metadata(&previous) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(io_error(
                    "could not inspect the previous output path",
                    source,
                ));
            }
            Ok(_) => {
                return Err(io_error(
                    "the previous output path became occupied during publication",
                    io::Error::from(io::ErrorKind::AlreadyExists),
                ));
            }
        }
        let existing = output.existing_output()?;
        self.cancellation.ensure_active()?;
        match existing {
            ExistingOutput::Absent | ExistingOutput::Empty => {
                if matches!(existing, ExistingOutput::Empty) {
                    fs::remove_dir(&output.boundary.output).map_err(|source| {
                        io_error("could not replace the empty output directory", source)
                    })?;
                }
                rename(&staging_path, &output.boundary.output)
                    .map_err(|source| io_error("could not publish the site output", source))?;
            }
            ExistingOutput::Owned => {
                // After moving a valid tree, finish installation or restoration even
                // when cancellation arrives. The borrowed site lock covers both.
                rename(&output.boundary.output, &previous).map_err(|source| {
                    io_error("could not move the previous site output aside", source)
                })?;
                if let Err(publish_error) = rename(&staging_path, &output.boundary.output) {
                    if let Err(restore_error) = rename(&previous, &output.boundary.output) {
                        return Err(io_error(
                            "could not publish the site, and could not restore the previous site output",
                            restore_error,
                        ));
                    }
                    return Err(io_error("could not publish the site output", publish_error));
                }
                // The installed graph is committed; cleanup cannot revoke success.
                if let Err(error) = self.remove_previous(&previous) {
                    tracing::warn!(
                        error = %error,
                        output = %output.boundary.output.display(),
                        "published output; deferred cleanup of the previous tree"
                    );
                }
            }
        }
        Ok(())
    }
}

fn marker_matches(path: &Path, expected: &[u8]) -> io::Result<bool> {
    let mut marker = crate::filesystem::open_regular_file(path, false)?;
    let mut contents = Vec::new();
    marker.read_to_end(&mut contents)?;
    Ok(contents == expected)
}

/// Whether a directory holds no entries.
fn directory_is_empty(directory: &Path) -> io::Result<bool> {
    let mut entries = fs::read_dir(directory)?;
    entries.next().transpose().map(|entry| entry.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolvedSiteConfig;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn root_alias_output_uses_canonical_path() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let root = directory.path().join("site");
        fs::create_dir(&root).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let alias = directory.path().join("alias");
        symlink(&root, &alias).unwrap();

        for site_root in [&canonical_root, &alias] {
            let owned = OwnedOutputRoot::resolve(site_root, &alias.join("public"), []).unwrap();
            assert_eq!(owned.path(), canonical_root.join("public"));
            prepare(&owned).unwrap();
        }
    }

    fn site_lock(output: &OwnedOutputRoot) -> crate::build::SiteBuildLock {
        crate::output::tests::site_lock(&output.boundary.site_root)
    }

    fn prepare(output: &OwnedOutputRoot) -> Result<(), OutputRootError> {
        let lock = site_lock(output);
        output
            .write(&lock, &BuildCancellation::default())?
            .prepare()
    }

    fn prepared_workspace(output: &OwnedOutputRoot) -> PathBuf {
        let lock = site_lock(output);
        output
            .write(&lock, &BuildCancellation::default())
            .unwrap()
            .prepare_workspace_for_staging()
            .unwrap()
            .to_path_buf()
    }

    fn published_output(site_root: &Path, files: &[(&str, &str)]) -> OwnedOutputRoot {
        let output = OwnedOutputRoot::resolve(site_root, &site_root.join("public"), []).unwrap();
        let staging = tempfile::tempdir_in(site_root).unwrap();
        for (name, contents) in files {
            fs::write(staging.path().join(name), contents).unwrap();
        }
        replace(&output, staging).unwrap();
        output
    }

    fn replace(
        output: &OwnedOutputRoot,
        staging: tempfile::TempDir,
    ) -> Result<(), OutputRootError> {
        let lock = site_lock(output);
        output
            .write(&lock, &BuildCancellation::default())?
            .replace_with(staging)
    }

    fn resolve_configured_output(config: &ResolvedSiteConfig) -> anyhow::Result<OwnedOutputRoot> {
        resolve_site_output_root_with_inputs(config, std::iter::empty())
    }

    fn output_test_config(root: &Path) -> ResolvedSiteConfig {
        let root = root.canonicalize().unwrap();
        let mut config = crate::config::tests::load_test_config(&root, "");
        config.config_path = root.join("tola.toml");
        config.build.publish_dir = root.join("public");
        config.build.entry = root.join("site.typ");
        config.build.content_dir = root.join("content");
        config
    }

    fn assert_protected_input(label: &str, error: anyhow::Error, kind: &str) {
        assert!(
            matches!(
                error.downcast_ref::<OutputRootError>(),
                Some(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::ProtectedPath { kind: found, .. }
                )) if *found == kind
            ),
            "{label}: {error:#}"
        );
    }

    /// Every input the output root refuses, each named by the kind its diagnostic reports.
    #[test]
    fn output_rejects_protected_inputs() {
        use crate::config::section::IconCollectionSource;
        let dir = TempDir::new().unwrap();

        for source in [
            IconCollectionSource::LocalJson {
                path: dir.path().join("public/icons.json"),
            },
            IconCollectionSource::LocalSvgDir {
                path: dir.path().join("public/icons"),
            },
        ] {
            let mut config = output_test_config(dir.path());
            config.icons.collections.insert("brand".into(), source);
            let error = resolve_configured_output(&config).unwrap_err();
            assert_protected_input("icon source", error, "icon source");
        }

        let mut config = output_test_config(dir.path());
        config.config_path = config.build.publish_dir.join("tola.toml");
        let error = resolve_configured_output(&config).unwrap_err();
        assert_protected_input("config", error, "config");

        let mut config = output_test_config(dir.path());
        config.build.entry = config.build.publish_dir.join("entry.typ");
        let error = resolve_configured_output(&config).unwrap_err();
        assert_protected_input("build entry", error, "build entry");

        let mut config = output_test_config(dir.path());
        config.build.content_dir = config.build.publish_dir.join("content");
        let error = resolve_configured_output(&config).unwrap_err();
        assert_protected_input("content-dir", error, "content-dir");

        for generated in ["public", ".public-publish"] {
            let config = output_test_config(dir.path());
            let dynamic_read = config
                .get_root()
                .join(generated)
                .join("templates/document.typ");
            let error = resolve_site_output_root_with_inputs(
                &config,
                [("Typst read", dynamic_read.as_path())],
            )
            .unwrap_err();
            assert_protected_input("Typst read", error, "Typst read");
        }

        for (relative, author_file) in [
            (Path::new(".tola"), None),
            (Path::new(".tola/nested"), None),
            (Path::new(".tola/public"), Some("author file")),
        ] {
            let mut config = output_test_config(dir.path());
            config.build.publish_dir = config.get_root().join(relative);
            if let Some(author_file) = author_file {
                let internal = config.get_root().join(crate::filesystem::INTERNAL_DIR);
                fs::create_dir_all(&internal).unwrap();
                fs::write(&config.build.publish_dir, author_file).unwrap();
            }
            let error = resolve_configured_output(&config).unwrap_err();
            assert_protected_input("`.tola` directory", error, "`.tola` directory");
            if let Some(author_file) = author_file {
                assert_eq!(
                    fs::read_to_string(&config.build.publish_dir).unwrap(),
                    author_file
                );
            }
        }

        let output = dir.path().join("public");
        for (package_path, package_cache_path, kind) in [
            (Some(output.join("packages")), None, "Typst package"),
            (None, Some(output.join("cache")), "Typst package cache"),
        ] {
            let mut config = output_test_config(dir.path());
            config.package_locations = tola_typst::PackageLocations::discover(
                package_path.or_else(|| Some(dir.path().join("safe-data"))),
                package_cache_path.or_else(|| Some(dir.path().join("safe-cache"))),
            )
            .unwrap();
            let error = resolve_configured_output(&config).unwrap_err();
            assert_protected_input("package root", error, kind);
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_rejects_linked_protected_inputs() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();

        let root = TempDir::new().unwrap();
        let mut config = output_test_config(root.path());
        fs::create_dir(&config.build.publish_dir).unwrap();
        let source = config.get_root().join("brand");
        symlink(&config.build.publish_dir, &source).unwrap();
        config.icons.collections.insert(
            "brand".into(),
            crate::config::section::IconCollectionSource::LocalSvgDir { path: source },
        );
        let error = resolve_configured_output(&config).unwrap_err();
        assert_protected_input("linked icon source", error, "icon source");

        let config = output_test_config(dir.path());
        let output = &config.build.publish_dir;
        fs::create_dir_all(output).unwrap();
        fs::write(output.join("template.typ"), "#let value = 1").unwrap();
        let read_alias = config.get_root().join("template.typ");
        symlink(output.join("template.typ"), &read_alias).unwrap();
        let error =
            resolve_site_output_root_with_inputs(&config, [("Typst read", read_alias.as_path())])
                .unwrap_err();
        assert_protected_input("linked Typst read", error, "Typst read");

        let internal = TempDir::new().unwrap();
        let config = output_test_config(internal.path());
        fs::create_dir(&config.build.publish_dir).unwrap();
        symlink(
            &config.build.publish_dir,
            config.get_root().join(crate::filesystem::INTERNAL_DIR),
        )
        .unwrap();
        let error = resolve_configured_output(&config).unwrap_err();
        assert_protected_input("linked `.tola` directory", error, "`.tola` directory");
    }

    #[cfg(windows)]
    #[test]
    fn output_rejects_case_aliased_internal() {
        let dir = TempDir::new().unwrap();
        let mut config = output_test_config(dir.path());
        fs::create_dir(config.get_root().join(crate::filesystem::INTERNAL_DIR)).unwrap();
        config.build.publish_dir = config.get_root().join(".TOLA/output");

        let error = resolve_configured_output(&config).unwrap_err();

        assert_protected_input("case-aliased `.tola`", error, "`.tola` directory");
    }

    /// Real path components decide overlap: an ancestor, the site root itself, an equal path, and
    /// a nested subtree are refused; paths sharing only text and disjoint siblings are accepted.
    #[test]
    fn output_overlap_follows_real_components() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("site");
        fs::create_dir(&root).unwrap();

        for output in [
            root.clone(),
            dir.path().to_path_buf(),
            dir.path().join("other"),
        ] {
            assert!(matches!(
                OwnedOutputRoot::resolve(&root, &output, []),
                Err(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::OutsideSite
                ))
            ));
        }

        for (output, protected, kind) in [
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
                OwnedOutputRoot::resolve(dir.path(), &output, [(kind, protected.as_path())]),
                Err(OutputRootError::Boundary(
                    crate::filesystem::OutputBoundaryError::ProtectedPath { kind: found, .. }
                )) if found == kind
            ));
        }

        for (output, protected, expected) in [
            ("public", "publicx", "public"),
            ("a", "ab", "a"),
            ("site", "site-assets", "site"),
            ("output", "outputs", "output"),
            ("site/public", "site/content", "site/public"),
        ] {
            let owned = OwnedOutputRoot::resolve(
                dir.path(),
                &dir.path().join(output),
                [("generated input", dir.path().join(protected).as_path())],
            )
            .unwrap_or_else(|error| panic!("{output} overlaps {protected}: {error}"));
            assert_eq!(
                owned.path(),
                dir.path().canonicalize().unwrap().join(expected)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn output_chain_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("site");
        let external = dir.path().join("external");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&external).unwrap();
        symlink(&external, root.join("public")).unwrap();
        let expected = root.canonicalize().unwrap().join("public");

        assert!(matches!(
            OwnedOutputRoot::resolve(&root, &root.join("public/site"), []),
            Err(OutputRootError::Boundary(crate::filesystem::OutputBoundaryError::Symlink { path, .. })) if path == expected
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_created_after_resolution_is_seen() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let external = dir.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("keep.txt"), "external data").unwrap();
        let owned = OwnedOutputRoot::resolve(dir.path(), &output, []).unwrap();
        let expected = owned.path().to_path_buf();
        symlink(&external, &output).unwrap();

        assert!(matches!(
            prepare(&owned),
            Err(OutputRootError::Boundary(crate::filesystem::OutputBoundaryError::Symlink { path, .. })) if path == expected
        ));
        assert_eq!(
            fs::read_to_string(external.join("keep.txt")).unwrap(),
            "external data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn protected_symlink_to_output_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let protected = dir.path().join("assets");
        fs::create_dir(&output).unwrap();
        symlink(&output, &protected).unwrap();

        assert!(matches!(
            OwnedOutputRoot::resolve(dir.path(), &output, [("asset", protected.as_path())]),
            Err(OutputRootError::Boundary(
                crate::filesystem::OutputBoundaryError::ProtectedPath { kind: "asset", .. }
            ))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rechecks_protected_symlink() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let protected = dir.path().join("assets");
        let owned = OwnedOutputRoot::resolve(dir.path(), &output, [("asset", protected.as_path())])
            .unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(output.join("keep.txt"), "generated data").unwrap();
        symlink(&output, &protected).unwrap();

        assert!(matches!(
            prepare(&owned),
            Err(OutputRootError::Boundary(
                crate::filesystem::OutputBoundaryError::ProtectedPath { kind: "asset", .. }
            ))
        ));
        assert_eq!(
            fs::read_to_string(output.join("keep.txt")).unwrap(),
            "generated data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn publisher_rechecks_workspace_identity() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let output = root.join("public");
        let content = root.join("content");
        fs::create_dir_all(content.join("previous")).unwrap();
        let source = content.join("previous/page.typ");
        fs::write(&source, "source").unwrap();
        let owned =
            OwnedOutputRoot::resolve(root, &output, [("content", content.as_path())]).unwrap();
        published_output(root, &[("index.html", "published")]);
        let staging = tempfile::tempdir_in(root).unwrap();
        fs::write(staging.path().join("index.html"), "candidate").unwrap();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();

        let workspace = owned.output_workspace().unwrap();
        let displaced = root.join("displaced-workspace");
        fs::rename(workspace, &displaced).unwrap();
        std::os::unix::fs::symlink(&content, workspace).unwrap();

        let published = writer.replace_with(staging);
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(output.join("index.html")).unwrap(), b"published");
        assert!(matches!(
            published,
            Err(OutputRootError::Boundary(
                crate::filesystem::OutputBoundaryError::Symlink { .. }
            ))
        ));
    }

    #[test]
    fn unpublished_output_is_preserved() {
        for marker in [None, Some("not an owner marker")] {
            let dir = TempDir::new().unwrap();
            let output = dir.path().join("public");
            fs::create_dir(&output).unwrap();
            fs::write(output.join("keep.txt"), "user data").unwrap();
            let owned = OwnedOutputRoot::resolve(dir.path(), &output, []).unwrap();
            if let Some(marker) = marker {
                // A record inside the output is the only one that speaks for it.
                let path = owner_marker_path(owned.path());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, marker).unwrap();
            }

            assert!(prepare(&owned).is_err(), "{marker:?}");
            assert_eq!(
                fs::read_to_string(output.join("keep.txt")).unwrap(),
                "user data"
            );
        }
    }

    #[test]
    fn replace_publishes_the_staged_tree() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("public");
        let owned = published_output(dir.path(), &[("stale.html", "stale")]);

        let workspace = owned.output_workspace().unwrap();
        let staging = tempfile::Builder::new()
            .prefix("staging-")
            .tempdir_in(workspace)
            .unwrap();
        fs::create_dir_all(staging.path().join("posts/rust")).unwrap();
        fs::write(staging.path().join("index.html"), "home").unwrap();
        fs::write(staging.path().join("posts/rust/index.html"), "rust").unwrap();

        replace(&owned, staging).unwrap();

        assert!(!output.join("stale.html").exists());
        assert_eq!(
            fs::read_to_string(output.join("index.html")).unwrap(),
            "home"
        );
        assert_eq!(
            fs::read_to_string(output.join("posts/rust/index.html")).unwrap(),
            "rust"
        );
        assert!(!owned.output_workspace().unwrap().join("previous").exists());
    }

    #[test]
    fn owner_marker_is_bound_to_one_output() {
        let dir = TempDir::new().unwrap();
        let output_a = dir.path().join("public-a");
        let output_b = dir.path().join("public-b");
        let owned_a = OwnedOutputRoot::resolve(dir.path(), &output_a, []).unwrap();
        replace(&owned_a, tempfile::tempdir_in(dir.path()).unwrap()).unwrap();

        fs::create_dir(&output_b).unwrap();
        fs::write(output_b.join("keep.txt"), "user data").unwrap();
        let owned_b = OwnedOutputRoot::resolve(dir.path(), &output_b, []).unwrap();
        let marker = owner_marker_path(&output_b);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::copy(owner_marker_path(&output_a), marker).unwrap();

        assert!(matches!(
            prepare(&owned_b),
            Err(OutputRootError::InvalidMarker { .. })
        ));
        assert_eq!(
            fs::read_to_string(output_b.join("keep.txt")).unwrap(),
            "user data"
        );
    }

    #[test]
    fn alternating_outputs_keep_ownership() {
        let directory = TempDir::new().unwrap();
        let first =
            OwnedOutputRoot::resolve(directory.path(), &directory.path().join("public"), [])
                .unwrap();
        let second =
            OwnedOutputRoot::resolve(directory.path(), &directory.path().join("preview"), [])
                .unwrap();
        for (output, contents) in [(&first, "first"), (&second, "second"), (&first, "updated")] {
            let staging = tempfile::tempdir_in(directory.path()).unwrap();
            fs::write(staging.path().join("index.html"), contents).unwrap();
            replace(output, staging).unwrap();
        }
        assert_eq!(
            fs::read(first.path().join("index.html")).unwrap(),
            b"updated"
        );
        assert_eq!(
            fs::read(second.path().join("index.html")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn foreign_site_lock_is_rejected() {
        let directory = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let output =
            OwnedOutputRoot::resolve(directory.path(), &directory.path().join("public"), [])
                .unwrap();
        let foreign =
            OwnedOutputRoot::resolve(other.path(), &other.path().join("public"), []).unwrap();
        let lock = site_lock(&foreign);
        assert!(matches!(
            output.write(&lock, &BuildCancellation::default()),
            Err(OutputRootError::WrongSiteLock)
        ));
        assert!(!output.path().exists());
    }

    #[test]
    fn foreign_workspace_is_preserved() {
        for marker in [None, Some("another owner")] {
            let directory = TempDir::new().unwrap();
            let output = directory.path().join("public");
            let workspace = crate::filesystem::publication_workspace(&output).unwrap();
            fs::create_dir_all(workspace.join("previous")).unwrap();
            fs::write(workspace.join("previous/keep.txt"), "author files").unwrap();
            if let Some(marker) = marker {
                fs::write(workspace.join(OWNER_MARKER_FILE), marker).unwrap();
            }
            let owned = OwnedOutputRoot::resolve(directory.path(), &output, []).unwrap();
            let lock = site_lock(&owned);

            assert!(matches!(
                owned.write(&lock, &BuildCancellation::default()),
                Err(OutputRootError::UnownedWorkspace { .. })
            ));
            assert_eq!(
                fs::read(workspace.join("previous/keep.txt")).unwrap(),
                b"author files"
            );
            assert!(!output.exists());
        }
    }

    /// A build killed between creating the workspace and recording its owner leaves this behind.
    #[test]
    fn empty_workspace_is_adopted() {
        let directory = TempDir::new().unwrap();
        let owned =
            OwnedOutputRoot::resolve(directory.path(), &directory.path().join("public"), [])
                .unwrap();
        let workspace = crate::filesystem::publication_workspace(owned.path()).unwrap();
        fs::create_dir(&workspace).unwrap();

        assert_eq!(prepared_workspace(&owned), workspace);
        assert_eq!(
            fs::read(workspace.join(OWNER_MARKER_FILE)).unwrap(),
            owned.marker_contents(WORKSPACE_MARKER_HEADER)
        );
    }

    #[test]
    fn abandoned_staging_is_reclaimed() {
        let directory = TempDir::new().unwrap();
        let owned =
            OwnedOutputRoot::resolve(directory.path(), &directory.path().join("public"), [])
                .unwrap();
        let workspace = prepared_workspace(&owned);
        let abandoned = workspace.join("staging-abandoned");
        fs::create_dir_all(abandoned.join("posts")).unwrap();
        fs::write(abandoned.join("posts/index.html"), "discarded candidate").unwrap();
        fs::write(workspace.join("keep.txt"), "author file").unwrap();

        assert_eq!(prepared_workspace(&owned), workspace);

        assert!(!abandoned.exists());
        assert_eq!(
            fs::read(workspace.join("keep.txt")).unwrap(),
            b"author file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_rejects_source_aliases() {
        let directory = TempDir::new().unwrap();
        let output = directory.path().join("public");
        let workspace = crate::filesystem::publication_workspace(&output).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("page.typ"), "source").unwrap();
        let alias = directory.path().join("page.typ");
        std::os::unix::fs::symlink(workspace.join("page.typ"), &alias).unwrap();

        assert!(matches!(
            OwnedOutputRoot::resolve(directory.path(), &output, [("Typst read", alias.as_path())]),
            Err(OutputRootError::Boundary(
                crate::filesystem::OutputBoundaryError::ProtectedPath {
                    kind: "Typst read",
                    ..
                }
            ))
        ));
        assert_eq!(fs::read(&alias).unwrap(), b"source");
    }

    #[test]
    fn first_install_failure_leaves_no_output() {
        let directory = TempDir::new().unwrap();
        let output = directory.path().join("public");
        let owned = OwnedOutputRoot::resolve(directory.path(), &output, []).unwrap();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();
        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        fs::write(staging.path().join("index.html"), "candidate").unwrap();

        let result = writer.replace_with_rename(staging, |_, _| {
            Err(io::Error::other("installation refused"))
        });
        assert!(result.is_err());
        assert!(!output.exists());
        assert!(!owned.output_workspace().unwrap().join("previous").exists());
    }

    #[test]
    fn cleanup_failure_keeps_committed_output() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "previous")]);
        let previous = owned.output_workspace().unwrap().join("previous");
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();
        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        fs::write(staging.path().join("index.html"), "committed").unwrap();

        writer
            .replace_with_rename(staging, |from, to| {
                fs::rename(from, to)?;
                if to == owned.path() {
                    fs::write(owner_marker_path(&previous), "foreign ownership")?;
                }
                Ok(())
            })
            .unwrap();

        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"committed"
        );
        assert_eq!(
            fs::read(owner_marker_path(&previous)).unwrap(),
            b"foreign ownership"
        );
    }

    #[test]
    fn cancelled_commit_keeps_published_output() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let canceller = crate::cancellation::BuildCanceller::default();
        let cancellation = canceller.token();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &cancellation).unwrap();
        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        fs::write(staging.path().join("index.html"), "candidate").unwrap();
        canceller.cancel();
        let error = writer.replace_with(staging).unwrap_err();
        assert!(crate::cancellation::is_cancelled(&error.into()));
        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"successful"
        );
    }

    #[test]
    fn installed_tree_is_not_reported_failed() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let canceller = crate::cancellation::BuildCanceller::default();
        let cancellation = canceller.token();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &cancellation).unwrap();
        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        fs::write(staging.path().join("index.html"), "candidate").unwrap();
        writer
            .replace_with_rename(staging, |from, to| {
                fs::rename(from, to)?;
                canceller.cancel();
                Ok(())
            })
            .unwrap();
        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"candidate"
        );
    }

    #[test]
    fn failed_install_restores_previous_output() {
        for fail_restore in [false, true] {
            let directory = TempDir::new().unwrap();
            let owned = published_output(directory.path(), &[("index.html", "successful")]);
            let staging = tempfile::tempdir_in(directory.path()).unwrap();
            fs::write(staging.path().join("index.html"), "candidate").unwrap();
            let mut renames = 0;
            let lock = site_lock(&owned);
            let result = owned
                .write(&lock, &BuildCancellation::default())
                .unwrap()
                .replace_with_rename(staging, |from, to| {
                    renames += 1;
                    if renames == 2 || (renames == 3 && fail_restore) {
                        return Err(io::Error::other("injected rename failure"));
                    }
                    fs::rename(from, to)
                });
            assert!(result.is_err());
            if fail_restore {
                assert!(!owned.path().exists());
                assert_eq!(
                    fs::read(
                        owned
                            .output_workspace()
                            .unwrap()
                            .join("previous/index.html")
                    )
                    .unwrap(),
                    b"successful"
                );
            } else {
                assert_eq!(
                    fs::read(owned.path().join("index.html")).unwrap(),
                    b"successful"
                );
            }
            let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();
            assert_eq!(
                fs::read(writer.path().join("index.html")).unwrap(),
                b"successful"
            );
        }
    }

    #[test]
    fn recovery_restores_previous_into_empty_output() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let previous = owned.output_workspace().unwrap().join("previous");
        fs::rename(owned.path(), &previous).unwrap();
        fs::create_dir(owned.path()).unwrap();
        let lock = site_lock(&owned);

        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();

        assert_eq!(
            fs::read(writer.path().join("index.html")).unwrap(),
            b"successful"
        );
        assert!(!previous.exists());
    }

    #[test]
    fn nonempty_destination_keeps_foreign_files() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let previous = owned.output_workspace().unwrap().join("previous");
        fs::rename(owned.path(), &previous).unwrap();
        fs::create_dir(owned.path()).unwrap();
        fs::write(owned.path().join("foreign.txt"), "author data").unwrap();
        let lock = site_lock(&owned);

        assert!(matches!(
            owned.write(&lock, &BuildCancellation::default()),
            Err(OutputRootError::Unowned { .. })
        ));
        assert_eq!(
            fs::read_to_string(owned.path().join("foreign.txt")).unwrap(),
            "author data"
        );
        assert_eq!(
            fs::read(previous.join("index.html")).unwrap(),
            b"successful"
        );
    }

    #[test]
    fn failed_recovery_keeps_previous_tree() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let previous = owned.output_workspace().unwrap().join("previous");
        fs::rename(owned.path(), &previous).unwrap();
        let marker = owner_marker_path(&previous);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, "invalid").unwrap();
        let lock = site_lock(&owned);
        assert!(owned.write(&lock, &BuildCancellation::default()).is_err());
        assert_eq!(
            fs::read(previous.join("index.html")).unwrap(),
            b"successful"
        );
        assert!(!owned.path().exists());
    }

    #[test]
    fn recovered_swap_keeps_installed_tree() {
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "previous")]);
        let previous = owned.output_workspace().unwrap().join("previous");
        fs::rename(owned.path(), &previous).unwrap();
        fs::create_dir(owned.path()).unwrap();
        fs::write(owned.path().join("index.html"), "installed").unwrap();
        owned.write_owner_marker(owned.path()).unwrap();
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();
        assert_eq!(
            fs::read(writer.path().join("index.html")).unwrap(),
            b"installed"
        );
        assert!(!previous.exists());
    }

    #[test]
    #[ignore = "subprocess helper for interrupted_swap_is_recovered"]
    fn publication_process_child() {
        use std::io::Write;
        let root = PathBuf::from(std::env::var_os("TOLA_PUBLICATION_TEST_ROOT").unwrap());
        let owned = OwnedOutputRoot::resolve(&root, &root.join("public"), []).unwrap();
        let cancellation = BuildCancellation::default();
        let lock = crate::build::SiteBuildLock::acquire_at_root(&root, &cancellation, || {
            println!("publication-blocked");
            io::stdout().flush().unwrap();
            let mut line = String::new();
            io::stdin().read_line(&mut line).unwrap();
        })
        .unwrap();
        let writer = owned.write(&lock, &cancellation).unwrap();
        assert_eq!(
            fs::read(writer.path().join("index.html")).unwrap(),
            b"successful"
        );
        let staging = tempfile::tempdir_in(&root).unwrap();
        fs::write(staging.path().join("index.html"), "candidate").unwrap();
        let mut renames = 0;
        let result = writer.replace_with_rename(staging, |from, to| {
            renames += 1;
            if renames == 2 {
                Err(io::Error::other("injected install failure"))
            } else {
                fs::rename(from, to)
            }
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"successful"
        );
    }

    #[test]
    fn interrupted_swap_is_recovered() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};
        let directory = TempDir::new().unwrap();
        let owned = published_output(directory.path(), &[("index.html", "successful")]);
        let lock = site_lock(&owned);
        let writer = owned.write(&lock, &BuildCancellation::default()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "output::root::tests::publication_process_child",
                "--ignored",
                "--nocapture",
            ])
            .env("TOLA_PUBLICATION_TEST_ROOT", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                stdout.read_line(&mut line).unwrap(),
                0,
                "child exited before contending on lock"
            );
            if line.contains("publication-blocked") {
                break;
            }
        }
        let previous = owned.output_workspace().unwrap().join("previous");
        fs::rename(owned.path(), &previous).unwrap();
        drop(writer);
        drop(lock);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"continue\n")
            .unwrap();
        let mut remaining = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut remaining).unwrap();
        assert!(child.wait().unwrap().success(), "{remaining}");
        assert_eq!(
            fs::read(owned.path().join("index.html")).unwrap(),
            b"successful"
        );
    }
}
