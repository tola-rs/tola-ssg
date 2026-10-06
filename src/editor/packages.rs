//! Generated editor packages and their view of configured local packages.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::writes::FileWrites;
use std::borrow::Cow;

use tola_build::cancellation::BuildCancellation;
use tola_build::config::ResolvedSiteConfig;

pub(crate) const GENERATED_PACKAGE_DIRECTORY: &str =
    tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY;

pub(crate) fn generated_package_files() -> Vec<(PathBuf, Cow<'static, str>)> {
    let mut files = Vec::new();
    for package in tola_packages::resolvable_packages() {
        let spec = package.spec();
        let root = Path::new(GENERATED_PACKAGE_DIRECTORY)
            .join(spec.namespace.as_str())
            .join(spec.name.as_str())
            .join(spec.version.to_string());
        files.extend(
            package
                .files()
                .map(|(path, source)| (root.join(path), source)),
        );
    }
    files
}

pub(super) fn add_initial_files(writes: &mut FileWrites) -> Result<()> {
    writes.add_directory(PACKAGE_VIEW_DIRECTORY)?;
    for (path, source) in generated_package_files() {
        writes.create_file(path, source.into_owned())?;
    }
    Ok(())
}

pub(super) fn add_editor_package_replacements(writes: &mut FileWrites) -> Result<()> {
    for (path, source) in generated_package_files() {
        writes.replace_file(path, source.into_owned())?;
    }
    Ok(())
}

pub(crate) fn obsolete_package_paths(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    collect_obsolete_package_paths(root, &generated_package_files(), &Default::default())
}

/// One failure inside Tola's generated package mirror.
///
/// The sentence names what Tola was doing; the mirror's own path and the operating system's
/// wording stay in the cause chain the debug log records.
fn mirror_failure(operation: &str, path: &Path, source: io::Error) -> anyhow::Error {
    anyhow::Error::new(source)
        .context(format!(
            "at `{}`",
            crate::terminal::display_path_as_given(path)
        ))
        .context(format!(
            "Tola could not {operation}: check its permissions, then rerun `tola editor setup`"
        ))
}

/// One failure reading a configured package root, named by the option that declares it.
///
/// The root can live anywhere on this machine, so the sentence names the option the author wrote
/// instead of a host location.
fn package_directory_failure(option: &str, source: io::Error) -> anyhow::Error {
    anyhow::Error::new(source).context(format!(
        "Tola could not read the package directory {option} names; check the directory and its permissions, then rerun `tola editor setup`"
    ))
}

fn collect_obsolete_package_paths(
    root: &Path,
    generated: &[(PathBuf, Cow<'static, str>)],
    cancellation: &BuildCancellation,
) -> Result<Vec<PathBuf>> {
    cancellation.ensure_active()?;
    let mut package_root = root.join(GENERATED_PACKAGE_DIRECTORY);
    package_root.push("tola");
    let expected_files = generated
        .iter()
        .map(|(path, _)| path.as_path())
        .collect::<std::collections::BTreeSet<_>>();
    let expected_versions = generated
        .iter()
        .filter_map(|(path, _)| path.parent())
        .collect::<std::collections::BTreeSet<_>>();
    let mut obsolete = Vec::new();
    let names = match std::fs::read_dir(&package_root) {
        Ok(names) => names,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(obsolete),
        Err(error) => {
            return Err(mirror_failure(
                "read the generated package directory",
                &package_root,
                error,
            ));
        }
    };
    for name in names {
        cancellation.ensure_active()?;
        let name = name
            .map_err(|error| {
                mirror_failure("read the generated package directory", &package_root, error)
            })?
            .path();
        let metadata = std::fs::symlink_metadata(&name).map_err(|error| {
            mirror_failure("read the generated package directory", &name, error)
        })?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            obsolete.push(name);
            continue;
        }
        let versions = std::fs::read_dir(&name).map_err(|error| {
            mirror_failure("read the generated package directory", &name, error)
        })?;
        let mut retained = false;
        for version in versions {
            cancellation.ensure_active()?;
            let version = version
                .map_err(|error| {
                    mirror_failure("read the generated package directory", &name, error)
                })?
                .path();
            let metadata = std::fs::symlink_metadata(&version).map_err(|error| {
                mirror_failure("read the generated package directory", &version, error)
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                obsolete.push(version);
                continue;
            }
            let relative = version
                .strip_prefix(root)
                .expect("package directory is below the site root");
            if expected_versions.contains(relative) {
                retained = true;
                let (_, nested) =
                    collect_obsolete_children(root, &version, &expected_files, cancellation)?;
                obsolete.extend(nested);
            } else {
                obsolete.push(version);
            }
        }
        if !retained {
            obsolete.push(name);
        }
    }
    obsolete.sort();
    Ok(obsolete)
}

/// Collect every path below one retained version that the generated files do not name, and
/// report whether any generated file lives below the directory: a directory without one is
/// itself obsolete.
fn collect_obsolete_children(
    root: &Path,
    directory: &Path,
    expected_files: &BTreeSet<&Path>,
    cancellation: &BuildCancellation,
) -> Result<(bool, Vec<PathBuf>)> {
    let mut retained = false;
    let mut obsolete = Vec::new();
    for child in std::fs::read_dir(directory)
        .map_err(|error| mirror_failure("read the generated package directory", directory, error))?
    {
        cancellation.ensure_active()?;
        let child = child
            .map_err(|error| {
                mirror_failure("read the generated package directory", directory, error)
            })?
            .path();
        let metadata = std::fs::symlink_metadata(&child).map_err(|error| {
            mirror_failure("read the generated package directory", &child, error)
        })?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            let (nested_retained, nested) =
                collect_obsolete_children(root, &child, expected_files, cancellation)?;
            if nested_retained {
                retained = true;
                obsolete.extend(nested);
            } else {
                obsolete.push(child);
            }
        } else if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            let relative = child
                .strip_prefix(root)
                .expect("package file is below the site root");
            if expected_files.contains(relative) {
                retained = true;
            } else {
                obsolete.push(child);
            }
        } else {
            obsolete.push(child);
        }
    }
    Ok((retained, obsolete))
}

pub(super) fn ensure_generated_package_paths_are_safe(root: &Path) -> Result<()> {
    check_generated_package_paths(root, &generated_package_files(), &Default::default())
}

fn check_generated_package_paths(
    root: &Path,
    generated: &[(PathBuf, Cow<'static, str>)],
    cancellation: &BuildCancellation,
) -> Result<()> {
    for (relative, _) in generated {
        let mut path = root.to_path_buf();
        let component_count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            cancellation.ensure_active()?;
            path.push(component);
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(mirror_failure(
                        "read the generated package directory",
                        &path,
                        error,
                    ));
                }
            };
            let displayed = crate::terminal::display_path_as_given(
                &relative.components().take(index + 1).collect::<PathBuf>(),
            );
            if metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "`{displayed}` is a symbolic link; remove it, then rerun `tola editor setup`"
                );
            }
            let final_component = index + 1 == component_count;
            if (!final_component && !metadata.is_dir()) || (final_component && !metadata.is_file())
            {
                anyhow::bail!(
                    "`{displayed}` is not a {}; move the existing path aside, then rerun `tola editor setup`",
                    if final_component { "file" } else { "directory" }
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn remove_obsolete_package_paths(root: &Path) -> Result<usize> {
    let obsolete = obsolete_package_paths(root)?;
    for path in obsolete.iter().rev() {
        remove_owned_path(path)?;
    }
    Ok(obsolete.len())
}

/// The Tola-owned package directory editors read builtin packages from, not an LSP transport.
pub(super) const PACKAGE_VIEW_DIRECTORY: &str = ".tola/editor-packages";

/// Links are read inputs: setup never modifies their local-package targets, and
/// Tola's own service uses the embedded package provider rather than this directory.
pub(crate) struct EditorPackageInputs {
    root: PathBuf,
    directories: BTreeSet<PathBuf>,
    links: BTreeMap<PathBuf, PathBuf>,
}

pub(crate) fn initial_package_inputs(
    root: &Path,
    packages: &tola_typst::PackageLocations,
    cancellation: &BuildCancellation,
) -> Result<EditorPackageInputs> {
    package_inputs(
        root,
        packages,
        &tola_typst::SourceBoundary::new(root, false)
            .excluding(root.join(tola_build::filesystem::INTERNAL_DIR)),
        cancellation,
    )
}

pub(crate) fn prepare_package_inputs(
    config: &ResolvedSiteConfig,
    resources: &tola_build::BuildResources,
) -> Result<EditorPackageInputs> {
    package_inputs(
        config.get_root(),
        &resources.package_locations(config),
        &resources.source_boundary(config),
        &Default::default(),
    )
}

fn package_inputs(
    root: &Path,
    packages: &tola_typst::PackageLocations,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &BuildCancellation,
) -> Result<EditorPackageInputs> {
    cancellation.ensure_active()?;
    let mut inputs = EditorPackageInputs {
        root: root.join(PACKAGE_VIEW_DIRECTORY),
        directories: BTreeSet::from([PathBuf::new()]),
        links: BTreeMap::new(),
    };
    for package in tola_packages::resolvable_packages() {
        cancellation.ensure_active()?;
        let spec = package.spec();
        let namespace = PathBuf::from(spec.namespace.as_str());
        let package = namespace.join(spec.name.as_str());
        inputs.directories.insert(namespace);
        inputs.directories.insert(package.clone());
        let version = package.join(spec.version.to_string());
        inputs.links.insert(
            version.clone(),
            root.join(GENERATED_PACKAGE_DIRECTORY).join(version),
        );
    }
    // Package roots in search order. The first root that provides an identity keeps it,
    // so the view resolves exactly what a build resolves.
    let declared = packages
        .declared()
        .iter()
        .map(|location| (location, "`vendor.path`"));
    let user = packages
        .data()
        .map(|location| (location, "`--package-path`"))
        .into_iter();
    for (local, option) in declared.chain(user) {
        let local = local.root();
        let local_identity = tola_build::filesystem::FilesystemSourceIdentity::from_path(local);
        let view_identity =
            tola_build::filesystem::FilesystemSourceIdentity::from_path(&inputs.root);
        if local_identity.intersects(&view_identity) {
            let root = inputs
                .root
                .parent()
                .expect("editor package root has a parent");
            let local = local.strip_prefix(root).unwrap_or(local);
            bail!(
                "the package directory `{}` overlaps Tola's editor package directory `{PACKAGE_VIEW_DIRECTORY}`; point {option} at a directory outside it",
                crate::terminal::display_path_as_given(local),
            );
        }
        for namespace in child_directories(local, option, boundary, cancellation)? {
            cancellation.ensure_active()?;
            let name = namespace
                .file_name()
                .context("package namespace has no name")?;
            let relative = PathBuf::from(name);
            inputs.directories.insert(relative.clone());
            for package in child_directories(&namespace, option, boundary, cancellation)? {
                cancellation.ensure_active()?;
                let name = package.file_name().context("local package has no name")?;
                let relative = relative.join(name);
                inputs.directories.insert(relative.clone());
                for installed in child_directories(&package, option, boundary, cancellation)? {
                    cancellation.ensure_active()?;
                    let name = installed
                        .file_name()
                        .context("package version has no name")?;
                    // Embedded and earlier versions win, other versions remain available.
                    inputs.links.entry(relative.join(name)).or_insert(installed);
                }
            }
        }
    }
    inputs.check()?;
    Ok(inputs)
}

impl EditorPackageInputs {
    fn check(&self) -> Result<()> {
        // Never traverse a site-owned symlink while planning or installing inputs.
        let internal = self
            .root
            .parent()
            .expect("editor package root has a parent");
        for (label, path) in [
            (".tola", internal),
            (PACKAGE_VIEW_DIRECTORY, self.root.as_path()),
        ] {
            match fs::symlink_metadata(path) {
                Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                    bail!(
                        "`{label}` is not a directory; move it aside, then rerun `tola editor setup`"
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(anyhow::Error::new(error).context(format!(
                        "Tola could not read `{label}`; check its permissions, then rerun `tola editor setup`"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every directory the view holds, sorted, including the view root itself.
    pub(crate) fn view_directories(&self) -> impl Iterator<Item = PathBuf> {
        self.directories.iter().map(|relative| {
            // `join("")` keeps a trailing separator; the view root is spelled without one.
            if relative.as_os_str().is_empty() {
                self.root.clone()
            } else {
                self.root.join(relative)
            }
        })
    }

    /// Every link the view holds, sorted by its path below the view root.
    pub(crate) fn view_links(&self) -> impl Iterator<Item = (PathBuf, &Path)> {
        self.links
            .iter()
            .map(|(relative, target)| (self.root.join(relative), target.as_path()))
    }

    pub(crate) fn apply(
        &self,
        cancellation: &tola_build::cancellation::BuildCancellation,
    ) -> Result<()> {
        cancellation.ensure_active()?;
        self.check()?;
        synchronize_directory(
            &self.root,
            Path::new(""),
            &self.directories,
            &self.links,
            cancellation,
        )
    }
}

fn child_directories(
    root: &Path,
    option: &str,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &BuildCancellation,
) -> Result<Vec<PathBuf>> {
    cancellation.ensure_active()?;
    boundary.check(root)?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(package_directory_failure(option, error)),
    };
    let mut directories = Vec::new();
    for entry in entries {
        cancellation.ensure_active()?;
        let path = entry
            .map_err(|error| package_directory_failure(option, error))?
            .path();
        boundary.check(&path)?;
        if fs::metadata(&path)
            .map_err(|error| package_directory_failure(option, error))?
            .is_dir()
        {
            directories.push(path);
        }
    }
    directories.sort();
    Ok(directories)
}

fn expected_children(
    relative: &Path,
    directories: &BTreeSet<PathBuf>,
    links: &BTreeMap<PathBuf, PathBuf>,
) -> BTreeSet<std::ffi::OsString> {
    directories
        .iter()
        .chain(links.keys())
        .filter_map(|path| {
            path.strip_prefix(relative)
                .ok()?
                .components()
                .next()
                .map(|component| component.as_os_str().to_os_string())
        })
        .collect()
}

fn link_matches(path: &Path, metadata: &fs::Metadata, target: &Path) -> Result<bool> {
    if !metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let existing = fs::read_link(path)
        .map_err(|error| mirror_failure("read the editor package directory", path, error))?;
    Ok(existing == target)
}

/// Make the mirror below `root` hold exactly the planned directories and links.
fn synchronize_directory(
    root: &Path,
    relative: &Path,
    directories: &BTreeSet<PathBuf>,
    links: &BTreeMap<PathBuf, PathBuf>,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<()> {
    cancellation.ensure_active()?;
    let directory = root.join(relative);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            remove_owned_path(&directory)?;
            fs::create_dir_all(&directory).map_err(|error| {
                mirror_failure("create the editor package directory", &directory, error)
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| {
                mirror_failure("create the editor package directory", &directory, error)
            })?
        }
        Err(error) => {
            return Err(mirror_failure(
                "read the editor package directory",
                &directory,
                error,
            ));
        }
    }
    let children = expected_children(relative, directories, links);
    for entry in fs::read_dir(&directory)
        .map_err(|error| mirror_failure("read the editor package directory", &directory, error))?
    {
        cancellation.ensure_active()?;
        let entry = entry.map_err(|error| {
            mirror_failure("read the editor package directory", &directory, error)
        })?;
        if !children.contains(&entry.file_name()) {
            remove_owned_path(&entry.path())?;
        }
    }
    for name in children {
        cancellation.ensure_active()?;
        let child = relative.join(name);
        let path = root.join(&child);
        if let Some(target) = links.get(&child) {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if link_matches(&path, &metadata, target)? => {
                    continue;
                }
                Ok(_) => remove_owned_path(&path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(mirror_failure(
                        "read the editor package directory",
                        &path,
                        error,
                    ));
                }
            }
            crate::sys::link_directory(target, &path).with_context(|| {
                format!(
                    "Tola could not link a local package into `{PACKAGE_VIEW_DIRECTORY}`; check that the directory is writable, then rerun `tola editor setup`"
                )
            })?;
        } else {
            synchronize_directory(root, &child, directories, links, cancellation)?;
        }
    }
    Ok(())
}

fn remove_owned_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| mirror_failure("read the editor package directory", path, error))?;
    if crate::sys::link_removes_as_directory(&metadata.file_type()) {
        fs::remove_dir(path).map_err(|error| {
            mirror_failure("remove an obsolete editor package entry", path, error)
        })?;
        return Ok(());
    }
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|error| {
            mirror_failure("remove an obsolete editor package entry", path, error)
        })?;
    } else {
        fs::remove_file(path).map_err(|error| {
            mirror_failure("remove an obsolete editor package entry", path, error)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::link_directory;

    #[cfg(unix)]
    /// The site configuration `path` selects under `packages`, with no overrides.
    fn loaded_config(
        path: &Path,
        packages: tola_typst::PackageLocations,
    ) -> crate::config::LoadedConfig {
        crate::config::load(
            Some(path),
            tola_build::InputScope::Online,
            packages,
            &crate::config::ConfigOverrides::default(),
        )
        .unwrap()
    }

    #[test]
    fn cancelled_initial_inputs_stop_preparation() {
        let directory = tempfile::tempdir().unwrap();
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();
        let error = initial_package_inputs(
            directory.path(),
            &tola_typst::PackageLocations::default(),
            &canceller.token(),
        )
        .err()
        .expect("cancelled preparation must stop");

        assert!(error.is::<tola_build::cancellation::BuildCancelled>());
    }

    #[test]
    fn obsolete_package_versions_are_removed() {
        let directory = tempfile::tempdir().unwrap();
        let obsolete = directory
            .path()
            .join(".tola/builtin-packages/tola/source/9.9.9");
        std::fs::create_dir_all(&obsolete).unwrap();
        std::fs::write(obsolete.join("lib.typ"), "old").unwrap();
        let unrelated = directory.path().join("keep.txt");
        std::fs::write(&unrelated, "keep").unwrap();

        assert!(remove_obsolete_package_paths(directory.path()).unwrap() >= 1);
        assert!(!obsolete.exists());
        assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep");
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn obsolete_link_removal_spares_its_target() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let target_file = external.path().join("lib.typ");
        fs::write(&target_file, "external package").unwrap();
        let obsolete = directory
            .path()
            .join(".tola/builtin-packages/tola/source/9.9.9");
        fs::create_dir_all(obsolete.parent().unwrap()).unwrap();
        link_directory(external.path(), &obsolete).unwrap();

        remove_obsolete_package_paths(directory.path()).unwrap();

        assert_eq!(
            fs::symlink_metadata(&obsolete).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(external.path().is_dir());
        assert_eq!(fs::read_to_string(target_file).unwrap(), "external package");
    }

    #[test]
    #[cfg(unix)]
    fn expected_package_symlink_is_rejected() {
        let external = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "").unwrap();
        let loaded = loaded_config(&path, tola_typst::PackageLocations::default());
        let config = loaded.config();
        let package = config
            .get_root()
            .join(".tola/builtin-packages/tola/source/0.0.0");
        std::fs::create_dir_all(package.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(external.path(), &package).unwrap();
        let directory = crate::editor::EditorDirectory::from_config(config).unwrap();
        let package_inputs = prepare_package_inputs(config, &Default::default()).unwrap();
        let error = crate::editor::setup(&directory, &[], package_inputs)
            .err()
            .expect("symlinked generated package path unexpectedly accepted");

        assert!(error.to_string().contains("symbolic link"));
        assert!(std::fs::read_dir(external.path()).unwrap().next().is_none());
    }

    #[test]
    fn extra_package_files_are_obsolete() {
        let directory = tempfile::tempdir().unwrap();
        let version = directory
            .path()
            .join(".tola/builtin-packages/tola/source/0.0.0");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("lib.typ"), "current").unwrap();
        std::fs::write(version.join("typst.toml"), "current").unwrap();
        let extra = version.join("extra.typ");
        std::fs::write(&extra, "extra").unwrap();

        let obsolete = obsolete_package_paths(directory.path()).unwrap();

        assert!(obsolete.contains(&extra));
        assert!(!obsolete.contains(&version));
    }

    /// A generated directory below a version stays: only paths the generated files do not name
    /// are removed.
    #[test]
    fn nested_generated_files_are_not_obsolete() {
        let directory = tempfile::tempdir().unwrap();
        let version = directory
            .path()
            .join(".tola/builtin-packages/tola/code/0.0.0");
        let theme = version.join("code-themes/zenburn.tmTheme");
        std::fs::create_dir_all(theme.parent().unwrap()).unwrap();
        std::fs::write(version.join("lib.typ"), "code").unwrap();
        std::fs::write(&theme, "theme").unwrap();
        let generated = vec![
            (
                PathBuf::from(".tola/builtin-packages/tola/code/0.0.0/lib.typ"),
                Cow::Borrowed("code"),
            ),
            (
                PathBuf::from(".tola/builtin-packages/tola/code/0.0.0/code-themes/zenburn.tmTheme"),
                Cow::Borrowed("theme"),
            ),
        ];

        let obsolete =
            collect_obsolete_package_paths(directory.path(), &generated, &Default::default())
                .unwrap();

        assert!(obsolete.is_empty(), "{obsolete:?}");
    }

    /// The generated views hold every package the compiler resolves, so a definition into any
    /// builtin package has a file an editor can open.
    #[test]
    fn generated_files_cover_packages() {
        let files = generated_package_files();

        for package in tola_packages::resolvable_packages() {
            let spec = package.spec();
            let root = Path::new(GENERATED_PACKAGE_DIRECTORY)
                .join(spec.namespace.as_str())
                .join(spec.name.as_str())
                .join(spec.version.to_string());
            assert!(
                files.iter().any(|(path, _)| path.starts_with(&root)),
                "`{spec}` is missing from the generated views"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn editor_inputs_prefer_embedded_versions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        fs::write(&path, "").unwrap();
        let local = tempfile::tempdir().unwrap();
        for relative in [
            "local/example/0.1.0",
            "tola/site/0.0.0",
            "tola/site/4.0.0",
            "tola/custom/0.1.0",
        ] {
            let directory = local.path().join(relative);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("lib.typ"), relative).unwrap();
        }
        let packages = tola_typst::PackageLocations::from_absolute_roots(
            Some(local.path().to_path_buf()),
            None,
        )
        .unwrap();
        let loaded = loaded_config(&path, packages);
        let config = loaded.config();
        let inputs = prepare_package_inputs(config, &Default::default()).unwrap();
        assert!(!directory.path().join(".tola").exists());
        let mut writes = FileWrites::new(config.get_root()).unwrap();
        add_editor_package_replacements(&mut writes).unwrap();
        writes.apply(&Default::default()).unwrap();
        inputs.apply(&Default::default()).unwrap();

        assert_eq!(
            fs::read_to_string(inputs.root.join("local/example/0.1.0/lib.typ")).unwrap(),
            "local/example/0.1.0"
        );
        assert_eq!(
            fs::read_to_string(inputs.root.join("tola/site/4.0.0/lib.typ")).unwrap(),
            "tola/site/4.0.0"
        );
        assert_eq!(
            fs::read_to_string(inputs.root.join("tola/custom/0.1.0/lib.typ")).unwrap(),
            "tola/custom/0.1.0"
        );
        let mirrored = fs::read_to_string(inputs.root.join("tola/site/0.0.0/lib.typ")).unwrap();
        assert_ne!(
            mirrored, "tola/site/0.0.0",
            "the local copy must not answer for a builtin version"
        );
        assert_eq!(
            mirrored,
            tola_packages::TolaPackage::Site
                .file(Path::new("lib.typ"))
                .expect("the builtin site package has lib.typ")
        );

        fs::write(
            local.path().join("local/example/0.1.0/lib.typ"),
            "edited on disk",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(inputs.root.join("local/example/0.1.0/lib.typ")).unwrap(),
            "edited on disk"
        );
        fs::remove_dir_all(local.path().join("local")).unwrap();
        let refreshed = prepare_package_inputs(config, &Default::default()).unwrap();
        refreshed.apply(&Default::default()).unwrap();
        assert!(fs::symlink_metadata(inputs.root.join("local")).is_err());
        assert!(local.path().join("tola/site/0.0.0/lib.typ").is_file());
    }

    #[test]
    #[cfg(unix)]
    fn unsafe_package_root_aborts_setup() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("tola.toml");
        fs::write(&config_path, "").unwrap();
        fs::create_dir(directory.path().join(".tola")).unwrap();
        link_directory(
            external.path(),
            &directory.path().join(PACKAGE_VIEW_DIRECTORY),
        )
        .unwrap();
        let config = loaded_config(&config_path, tola_typst::PackageLocations::default());
        let error = prepare_package_inputs(config.config(), &Default::default())
            .err()
            .expect("symlinked package view unexpectedly accepted");

        assert!(error.to_string().contains("is not a directory"), "{error}");
    }
}
