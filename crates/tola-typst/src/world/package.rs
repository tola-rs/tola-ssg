//! Typst package location and retrieval.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(feature = "network")]
use std::sync::Arc;

use typst::diag::{FileError, FileResult, PackageError};
pub use typst::syntax::package::{PackageSpec, PackageVersion};
#[cfg(feature = "network")]
use typst_kit::downloader::SystemDownloader;
use typst_kit::packages::FsPackages;
#[cfg(feature = "network")]
use typst_kit::packages::UniversePackages;

const PACKAGE_PATH_ENV: &str = "TYPST_PACKAGE_PATH";
const PACKAGE_CACHE_PATH_ENV: &str = "TYPST_PACKAGE_CACHE_PATH";

/// Why a Typst package root was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageLocationSource {
    /// Declared by the caller, before every standard Typst root.
    Declared,
    /// Supplied directly by the caller.
    Explicit,
    /// Read from Typst's package environment variable.
    Environment,
    /// Derived from the platform's standard directories.
    System,
}

/// One frozen Typst package root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageLocation {
    root: PathBuf,
    source: PackageLocationSource,
}

impl PackageLocation {
    /// Return the absolute logical root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return why this root was selected.
    pub fn source(&self) -> PackageLocationSource {
        self.source
    }
}

/// Effective Typst package roots for one process invocation.
///
/// Declared roots take precedence over explicit paths, which take precedence over environment
/// paths, which take precedence over Typst's platform defaults. Relative explicit and environment
/// paths are resolved against the invocation's current directory exactly once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct PackageLocations {
    declared: Vec<PackageLocation>,
    data: Option<PackageLocation>,
    cache: Option<PackageLocation>,
}

impl PackageLocations {
    /// Freeze the effective roots for this process invocation.
    pub fn discover(
        explicit_data: Option<PathBuf>,
        explicit_cache: Option<PathBuf>,
    ) -> FileResult<Self> {
        Self::discover_with(
            explicit_data,
            explicit_cache,
            |name| std::env::var_os(name),
            FsPackages::system_data().map(|root| root.path().to_path_buf()),
            FsPackages::system_cache().map(|root| root.path().to_path_buf()),
            std::env::current_dir,
        )
    }

    /// Construct already-frozen absolute package roots.
    pub fn from_absolute_roots(data: Option<PathBuf>, cache: Option<PathBuf>) -> FileResult<Self> {
        let location = |root: PathBuf| {
            if !root.is_absolute() {
                return Err(FileError::AccessDenied);
            }
            Ok(PackageLocation {
                root,
                source: PackageLocationSource::Explicit,
            })
        };
        Ok(Self {
            declared: Vec::new(),
            data: data.map(location).transpose()?,
            cache: cache.map(location).transpose()?,
        })
    }

    /// Return a copy that searches `root` before every standard package root.
    ///
    /// Declared roots keep their declaration order: each one is searched before the next, and all
    /// of them before the user's local packages and the downloaded-package cache. The root is
    /// absolute because the caller resolves it before declaring it.
    pub fn with_declared_root(mut self, root: PathBuf) -> FileResult<Self> {
        if !root.is_absolute() {
            return Err(FileError::AccessDenied);
        }
        self.declared.push(PackageLocation {
            root,
            source: PackageLocationSource::Declared,
        });
        Ok(self)
    }

    /// Retain declared site roots while refusing both host tiers.
    pub fn without_host_roots(mut self) -> Self {
        self.data = None;
        self.cache = None;
        self
    }

    /// Replace declared roots without changing frozen host locations or their provenance.
    pub fn with_declared_roots(
        mut self,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> FileResult<Self> {
        self.declared.clear();
        for root in roots {
            self = self.with_declared_root(root)?;
        }
        Ok(self)
    }

    /// Every caller-declared root, in search order.
    pub fn declared(&self) -> &[PackageLocation] {
        &self.declared
    }

    /// Return the effective local package root.
    pub fn data(&self) -> Option<&PackageLocation> {
        self.data.as_ref()
    }

    /// Return the effective downloaded-package cache root.
    pub fn cache(&self) -> Option<&PackageLocation> {
        self.cache.as_ref()
    }

    /// Every root in search order: declared roots first, then the user's local packages, then the
    /// downloaded-package cache.
    fn tiers(&self) -> impl Iterator<Item = (PackageTier, &PackageLocation)> {
        self.declared
            .iter()
            .map(|location| (PackageTier::Declared, location))
            .chain(
                self.data
                    .iter()
                    .map(|location| (PackageTier::Data, location)),
            )
            .chain(
                self.cache
                    .iter()
                    .map(|location| (PackageTier::Cache, location)),
            )
    }

    fn discover_with(
        explicit_data: Option<PathBuf>,
        explicit_cache: Option<PathBuf>,
        environment: impl Fn(&str) -> Option<OsString>,
        system_data: Option<PathBuf>,
        system_cache: Option<PathBuf>,
        current_dir: impl FnOnce() -> std::io::Result<PathBuf>,
    ) -> FileResult<Self> {
        let data = select_location(explicit_data, environment(PACKAGE_PATH_ENV), system_data);
        let cache = select_location(
            explicit_cache,
            environment(PACKAGE_CACHE_PATH_ENV),
            system_cache,
        );
        let needs_current_dir = data.as_ref().is_some_and(|(path, _)| path.is_relative())
            || cache.as_ref().is_some_and(|(path, _)| path.is_relative());
        let current_dir = needs_current_dir
            .then(current_dir)
            .transpose()
            .map_err(|error| FileError::from_io(error, Path::new(".")))?;

        let freeze = |selected: Option<(PathBuf, PackageLocationSource)>| {
            selected.map(|(root, source)| PackageLocation {
                root: if root.is_absolute() {
                    root
                } else {
                    current_dir
                        .as_ref()
                        .expect("relative package root requires an invocation directory")
                        .join(root)
                },
                source,
            })
        };

        Ok(Self {
            declared: Vec::new(),
            data: freeze(data),
            cache: freeze(cache),
        })
    }
}

fn select_location(
    explicit: Option<PathBuf>,
    environment: Option<OsString>,
    system: Option<PathBuf>,
) -> Option<(PathBuf, PackageLocationSource)> {
    explicit
        .map(|path| (path, PackageLocationSource::Explicit))
        .or_else(|| {
            environment.map(|path| (PathBuf::from(path), PackageLocationSource::Environment))
        })
        .or_else(|| system.map(|path| (path, PackageLocationSource::System)))
}

/// Whether a resolver may download missing preview packages.
///
/// `AllowNetwork` exists only in a build with the `network` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageFetchPolicy {
    /// Allow Typst Universe downloads into the configured cache.
    #[cfg(feature = "network")]
    AllowNetwork,
    /// Never access the network or write downloaded packages.
    LocalOnly,
}

/// Package-root tier that was actually checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageTier {
    /// Package storage the caller declares.
    Declared,
    /// User-managed local package storage.
    Data,
    /// Downloaded-package cache.
    Cache,
}

/// What an actual package-directory check observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageAvailability {
    /// The candidate directory did not exist.
    Missing,
    /// The candidate existed and was a directory.
    Present,
    /// The candidate existed but was not a directory.
    NotDirectory,
    /// The filesystem did not permit a reliable observation.
    Unreadable,
}

/// One actual package-directory availability check.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageCheck {
    package: PackageSpec,
    tier: PackageTier,
    root: PathBuf,
    candidate: PathBuf,
    availability: PackageAvailability,
    canonical_target: Option<PathBuf>,
    selected: bool,
}

/// Sort package checks into the order evidence and retries report them.
///
/// The order is stable across runs: package identity, storage tier, candidate directory,
/// availability, then whether this run selected the candidate. Package checks for one build arrive
/// from several tiers and from before-build hooks, so every consumer sorts them the same way.
pub fn sort_package_checks(checks: &mut [PackageCheck]) {
    checks.sort_by_cached_key(|check| {
        (
            check.package.to_string(),
            check.tier as u8,
            check.candidate.clone(),
            check.availability as u8,
            check.selected,
        )
    });
}

impl PackageCheck {
    /// Return the checked package identity.
    pub fn package(&self) -> &PackageSpec {
        &self.package
    }

    /// Return the checked storage tier.
    pub fn tier(&self) -> PackageTier {
        self.tier
    }

    /// Return the root the candidate directory was checked in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the exact candidate directory passed to the availability check.
    pub fn candidate(&self) -> &Path {
        &self.candidate
    }

    /// Return the observed directory availability.
    pub fn availability(&self) -> PackageAvailability {
        self.availability
    }

    /// Return the physical directory observed for a present candidate.
    pub fn canonical_target(&self) -> Option<&Path> {
        self.canonical_target.as_deref()
    }

    /// Whether this candidate became the package root for the read.
    pub fn was_selected(&self) -> bool {
        self.selected
    }
}

/// A package directory selected for a resolver read.
#[derive(Debug)]
pub struct PreparedPackage {
    directory: PathBuf,
    checks: Vec<PackageCheck>,
}

impl PreparedPackage {
    /// Return the selected physical package directory.
    ///
    /// Symbolic links in the logical candidate are resolved during selection.
    /// The original candidate remains available through [`Self::checks`].
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Return every availability check that determined the selection.
    pub fn checks(&self) -> &[PackageCheck] {
        &self.checks
    }

    pub(crate) fn into_directory_and_checks(self) -> (PathBuf, Vec<PackageCheck>) {
        (self.directory, self.checks)
    }
}

/// Package selection failed after these availability checks.
#[derive(Debug)]
pub struct PackagePreparationFailure {
    error: FileError,
    checks: Vec<PackageCheck>,
}

impl PackagePreparationFailure {
    /// Return every availability check completed before the failure.
    pub fn checks(&self) -> &[PackageCheck] {
        &self.checks
    }

    /// Consume the failure into its unchanged file error and checks.
    pub fn into_error_and_checks(self) -> (FileError, Vec<PackageCheck>) {
        (self.error, self.checks)
    }
}

impl std::fmt::Display for PackagePreparationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for PackagePreparationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Package storage owned by an explicit file resolver.
#[derive(Clone)]
pub struct PackageStore {
    locations: Result<PackageLocations, FileError>,
    fetch_policy: PackageFetchPolicy,
    boundary: crate::world::SourceBoundary,
    /// Typst Universe downloader, absent from a build without the `network` feature.
    #[cfg(feature = "network")]
    universe: Arc<UniversePackages>,
}

impl PackageStore {
    /// Create a store from already-frozen package locations.
    pub fn new(locations: PackageLocations, fetch_policy: PackageFetchPolicy) -> Self {
        Self::frozen(Ok(locations), fetch_policy)
    }

    /// Refuse package directories that would make generated or external files into sources.
    pub fn with_source_boundary(mut self, boundary: crate::world::SourceBoundary) -> Self {
        self.boundary = boundary;
        self
    }

    /// Create a store with an explicit registry User-Agent.
    #[cfg(feature = "network")]
    pub fn with_user_agent(
        locations: PackageLocations,
        fetch_policy: PackageFetchPolicy,
        user_agent: impl Into<String>,
    ) -> Self {
        Self {
            locations: Ok(locations),
            fetch_policy,
            boundary: crate::world::SourceBoundary::default(),
            universe: Arc::new(UniversePackages::new(SystemDownloader::new(
                user_agent.into(),
            ))),
        }
    }

    /// A store over frozen roots, or the invocation error that prevented discovery.
    #[cfg(feature = "network")]
    fn frozen(locations: FileResult<PackageLocations>, fetch_policy: PackageFetchPolicy) -> Self {
        Self {
            locations,
            fetch_policy,
            boundary: crate::world::SourceBoundary::default(),
            universe: Arc::new(UniversePackages::new(SystemDownloader::new(concat!(
                "tola-typst/",
                env!("CARGO_PKG_VERSION")
            )))),
        }
    }

    /// A store over frozen roots, or the invocation error that prevented discovery.
    #[cfg(not(feature = "network"))]
    fn frozen(locations: FileResult<PackageLocations>, fetch_policy: PackageFetchPolicy) -> Self {
        Self {
            locations,
            fetch_policy,
            boundary: crate::world::SourceBoundary::default(),
        }
    }

    /// Return the frozen locations, or the invocation error that prevented
    /// their discovery.
    pub fn locations(&self) -> FileResult<&PackageLocations> {
        self.locations.as_ref().map_err(Clone::clone)
    }

    /// Return the store's network policy.
    pub fn fetch_policy(&self) -> PackageFetchPolicy {
        self.fetch_policy
    }

    /// Select or retrieve a package directory.
    ///
    /// Tiers are searched in order: a caller-declared root, the user's local
    /// packages, then the downloaded-package cache. A download
    /// happens only in the cache tier, and only for the namespace Typst
    /// Universe serves.
    pub fn prepare(
        &self,
        package: &PackageSpec,
    ) -> Result<PreparedPackage, PackagePreparationFailure> {
        let locations = match &self.locations {
            Ok(locations) => locations,
            Err(error) => {
                return Err(PackagePreparationFailure {
                    error: error.clone(),
                    checks: Vec::new(),
                });
            }
        };
        let mut checks = Vec::new();

        for (tier, location) in locations.tiers() {
            match check_package_directory(package, tier, location.root(), &self.boundary) {
                PackageDirectoryCheck::Present {
                    mut check,
                    directory,
                } => {
                    check.selected = true;
                    checks.push(check);
                    return Ok(PreparedPackage { directory, checks });
                }
                PackageDirectoryCheck::Missing(check) => checks.push(check),
                PackageDirectoryCheck::Failed { check, error } => {
                    checks.push(check);
                    return Err(PackagePreparationFailure { error, checks });
                }
            }

            #[cfg(feature = "network")]
            if tier == PackageTier::Cache
                && self.fetch_policy == PackageFetchPolicy::AllowNetwork
                && package.namespace == UniversePackages::NAMESPACE
            {
                let mut archive =
                    self.universe
                        .package(package)
                        .map_err(|error| PackagePreparationFailure {
                            error: FileError::from(error),
                            checks: checks.clone(),
                        })?;
                FsPackages::new(location.root())
                    .store(package, |directory| {
                        archive.unpack(directory).map_err(|error| {
                            PackageError::MalformedArchive(Some(format!("{error}").into()))
                        })
                    })
                    .map_err(|error| PackagePreparationFailure {
                        error: FileError::from(error),
                        checks: checks.clone(),
                    })?;

                match check_package_directory(
                    package,
                    PackageTier::Cache,
                    location.root(),
                    &self.boundary,
                ) {
                    PackageDirectoryCheck::Present {
                        mut check,
                        directory,
                    } => {
                        checks.retain(|observed| {
                            observed.tier != PackageTier::Cache
                                || observed.candidate != check.candidate
                        });
                        check.selected = true;
                        checks.push(check);
                        return Ok(PreparedPackage { directory, checks });
                    }
                    PackageDirectoryCheck::Missing(check) => checks.push(check),
                    PackageDirectoryCheck::Failed { check, error } => {
                        checks.push(check);
                        return Err(PackagePreparationFailure { error, checks });
                    }
                }
            }
        }

        Err(PackagePreparationFailure {
            error: FileError::from(PackageError::NotFound(package.clone())),
            checks,
        })
    }

    /// Parse and prepare a package directory.
    pub fn prepare_package(
        &self,
        package: impl AsRef<str>,
    ) -> Result<PreparedPackage, PackagePreparationFailure> {
        let package =
            package
                .as_ref()
                .parse::<PackageSpec>()
                .map_err(|error| PackagePreparationFailure {
                    error: FileError::Other(Some(error.to_string().into())),
                    checks: Vec::new(),
                })?;
        self.prepare(&package)
    }
}

impl Default for PackageStore {
    /// A store over the discovered roots that never downloads: network access is opt-in.
    fn default() -> Self {
        Self::frozen(
            PackageLocations::discover(None, None),
            PackageFetchPolicy::LocalOnly,
        )
    }
}

enum PackageDirectoryCheck {
    Present {
        check: PackageCheck,
        directory: PathBuf,
    },
    Missing(PackageCheck),
    Failed {
        check: PackageCheck,
        error: FileError,
    },
}

fn check_package_directory(
    package: &PackageSpec,
    tier: PackageTier,
    root: &Path,
    boundary: &crate::world::SourceBoundary,
) -> PackageDirectoryCheck {
    let candidate = root
        .join(package.namespace.as_str())
        .join(package.name.as_str())
        .join(package.version.to_string());
    if let Err(error) = boundary.check(&candidate) {
        return PackageDirectoryCheck::Failed {
            check: PackageCheck {
                package: package.clone(),
                tier,
                root: root.to_path_buf(),
                candidate,
                availability: PackageAvailability::Unreadable,
                canonical_target: None,
                selected: false,
            },
            error,
        };
    }
    // A failed check names the package the author imported; its host directory
    // never reaches a rendered diagnostic.
    let reported = PathBuf::from(package.to_string());
    match std::fs::metadata(&candidate) {
        Ok(metadata) if metadata.is_dir() => match std::fs::canonicalize(&candidate) {
            Ok(directory) => PackageDirectoryCheck::Present {
                check: PackageCheck {
                    package: package.clone(),
                    tier,
                    root: root.to_path_buf(),
                    candidate,
                    availability: PackageAvailability::Present,
                    canonical_target: Some(directory.clone()),
                    selected: false,
                },
                directory,
            },
            Err(error) => {
                let check = PackageCheck {
                    package: package.clone(),
                    tier,
                    root: root.to_path_buf(),
                    candidate: candidate.clone(),
                    availability: PackageAvailability::Unreadable,
                    canonical_target: None,
                    selected: false,
                };
                PackageDirectoryCheck::Failed {
                    check,
                    error: FileError::from_io(error, &reported),
                }
            }
        },
        Ok(_) => {
            let check = PackageCheck {
                package: package.clone(),
                tier,
                root: root.to_path_buf(),
                candidate: candidate.clone(),
                availability: PackageAvailability::NotDirectory,
                canonical_target: None,
                selected: false,
            };
            PackageDirectoryCheck::Failed {
                check,
                error: FileError::Other(Some(
                    format!("could not read package `{package}`: its cache location holds a file, not a directory").into(),
                )),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            PackageDirectoryCheck::Missing(PackageCheck {
                package: package.clone(),
                tier,
                root: root.to_path_buf(),
                candidate,
                availability: PackageAvailability::Missing,
                canonical_target: None,
                selected: false,
            })
        }
        Err(error) => {
            let check = PackageCheck {
                package: package.clone(),
                tier,
                root: root.to_path_buf(),
                candidate: candidate.clone(),
                availability: PackageAvailability::Unreadable,
                canonical_target: None,
                selected: false,
            };
            PackageDirectoryCheck::Failed {
                check,
                error: FileError::from_io(error, &reported),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tempfile::TempDir;

    fn package() -> PackageSpec {
        "@preview/demo:0.1.0".parse().unwrap()
    }

    #[test]
    fn explicit_paths_outrank_environment() {
        let directory = TempDir::new().unwrap();
        let current_dir_calls = Cell::new(0);
        let locations = PackageLocations::discover_with(
            Some(PathBuf::from("explicit-data")),
            None,
            |name| match name {
                PACKAGE_PATH_ENV => Some(OsString::from("ignored-data")),
                PACKAGE_CACHE_PATH_ENV => Some(OsString::from("environment-cache")),
                _ => None,
            },
            Some(PathBuf::from("system-data")),
            Some(PathBuf::from("system-cache")),
            || {
                current_dir_calls.set(current_dir_calls.get() + 1);
                Ok(directory.path().to_path_buf())
            },
        )
        .unwrap();

        assert_eq!(current_dir_calls.get(), 1);
        assert_eq!(
            locations.data().unwrap().root(),
            directory.path().join("explicit-data")
        );
        assert_eq!(
            locations.data().unwrap().source(),
            PackageLocationSource::Explicit
        );
        assert_eq!(
            locations.cache().unwrap().root(),
            directory.path().join("environment-cache")
        );
        assert_eq!(
            locations.cache().unwrap().source(),
            PackageLocationSource::Environment
        );
    }

    #[test]
    fn selection_records_every_tier_checked() {
        for (name, present_tier, expected_checks) in [
            (
                "data tier present",
                PackageTier::Data,
                &[(PackageTier::Data, PackageAvailability::Present, true)][..],
            ),
            (
                "only cache tier present",
                PackageTier::Cache,
                &[
                    (PackageTier::Data, PackageAvailability::Missing, false),
                    (PackageTier::Cache, PackageAvailability::Present, true),
                ][..],
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let data = directory.path().join("data");
            let cache = directory.path().join("cache");
            let declared = directory.path().join("declared");
            let package_dir = match present_tier {
                PackageTier::Declared => declared.join("preview/demo/0.1.0"),
                PackageTier::Data => data.join("preview/demo/0.1.0"),
                PackageTier::Cache => cache.join("preview/demo/0.1.0"),
            };
            std::fs::create_dir_all(&package_dir).unwrap();
            let locations =
                PackageLocations::from_absolute_roots(Some(data.clone()), Some(cache.clone()))
                    .unwrap();
            let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

            let prepared = store.prepare(&package()).unwrap();

            assert_eq!(
                prepared.directory(),
                package_dir.canonicalize().unwrap(),
                "{name}"
            );
            assert_eq!(prepared.checks().len(), expected_checks.len(), "{name}");
            for (check, (tier, availability, selected)) in
                prepared.checks().iter().zip(expected_checks)
            {
                let tier_root = match tier {
                    PackageTier::Declared => &declared,
                    PackageTier::Data => &data,
                    PackageTier::Cache => &cache,
                };
                assert_eq!(check.tier(), *tier, "{name}");
                assert_eq!(
                    check.candidate(),
                    tier_root.join("preview/demo/0.1.0"),
                    "{name}"
                );
                assert_eq!(check.availability(), *availability, "{name}");
                assert_eq!(check.was_selected(), *selected, "{name}");
            }
        }
    }

    #[test]
    fn non_directory_candidate_is_terminal() {
        let directory = TempDir::new().unwrap();
        let data = directory.path().join("data");
        let cache = directory.path().join("cache");
        let candidate = data.join("preview/demo/0.1.0");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, b"not a directory").unwrap();
        std::fs::create_dir_all(cache.join("preview/demo/0.1.0")).unwrap();
        let locations = PackageLocations::from_absolute_roots(Some(data), Some(cache)).unwrap();
        let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

        let failure = store.prepare(&package()).unwrap_err();

        assert_eq!(failure.checks().len(), 1);
        assert_eq!(failure.checks()[0].tier(), PackageTier::Data);
        assert_eq!(
            failure.checks()[0].availability(),
            PackageAvailability::NotDirectory
        );
    }

    #[test]
    fn local_policy_never_creates_cache() {
        let directory = TempDir::new().unwrap();
        let cache = directory.path().join("cache");
        let locations = PackageLocations::from_absolute_roots(None, Some(cache.clone())).unwrap();
        let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

        let failure = store.prepare(&package()).unwrap_err();

        assert!(matches!(
            failure.error,
            FileError::Package(PackageError::NotFound(_))
        ));
        assert_eq!(failure.checks().len(), 1);
        assert!(!cache.exists());
    }

    #[test]
    fn declared_roots_precede_other_tiers() {
        for (name, declared) in [
            ("single declared root", &["declared"][..]),
            ("declaration order", &["overlay", "vendored"][..]),
        ] {
            let directory = TempDir::new().unwrap();
            let data = directory.path().join("data");
            let cache = directory.path().join("cache");
            for root in [&data, &cache] {
                std::fs::create_dir_all(root.join("preview/demo/0.1.0")).unwrap();
            }
            let locations = PackageLocations::from_absolute_roots(Some(data), Some(cache))
                .unwrap()
                .with_declared_roots(declared.iter().map(|root| directory.path().join(root)))
                .unwrap();
            for root in declared {
                std::fs::create_dir_all(directory.path().join(root).join("preview/demo/0.1.0"))
                    .unwrap();
            }
            let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

            let prepared = store.prepare(&package()).unwrap();

            assert_eq!(
                prepared.directory(),
                directory
                    .path()
                    .join(declared[0])
                    .join("preview/demo/0.1.0")
                    .canonicalize()
                    .unwrap(),
                "{name}"
            );
            assert_eq!(
                prepared
                    .checks()
                    .iter()
                    .map(|check| (check.tier(), check.availability(), check.was_selected()))
                    .collect::<Vec<_>>(),
                [(PackageTier::Declared, PackageAvailability::Present, true)],
                "{name}"
            );
        }
    }

    #[test]
    fn declared_root_fallback_keeps_order() {
        for (name, declared, package_root, expected) in [
            (
                "next declared root",
                &["overlay", "vendored"][..],
                "vendored",
                &[
                    (
                        "overlay",
                        PackageTier::Declared,
                        PackageAvailability::Missing,
                        false,
                    ),
                    (
                        "vendored",
                        PackageTier::Declared,
                        PackageAvailability::Present,
                        true,
                    ),
                ][..],
            ),
            (
                "data tier",
                &["declared"][..],
                "data",
                &[
                    (
                        "declared",
                        PackageTier::Declared,
                        PackageAvailability::Missing,
                        false,
                    ),
                    (
                        "data",
                        PackageTier::Data,
                        PackageAvailability::Present,
                        true,
                    ),
                ][..],
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let data = directory.path().join("data");
            let locations = PackageLocations::from_absolute_roots(Some(data), None)
                .unwrap()
                .with_declared_roots(declared.iter().map(|root| directory.path().join(root)))
                .unwrap();
            for root in declared {
                std::fs::create_dir_all(directory.path().join(root)).unwrap();
            }
            std::fs::create_dir_all(
                directory
                    .path()
                    .join(package_root)
                    .join("preview/demo/0.1.0"),
            )
            .unwrap();
            let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

            let prepared = store.prepare(&package()).unwrap();

            assert_eq!(
                prepared.directory(),
                directory
                    .path()
                    .join(package_root)
                    .join("preview/demo/0.1.0")
                    .canonicalize()
                    .unwrap(),
                "{name}"
            );
            assert_eq!(
                prepared
                    .checks()
                    .iter()
                    .map(|check| (
                        check.root().to_path_buf(),
                        check.tier(),
                        check.availability(),
                        check.was_selected()
                    ))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|(root, tier, availability, selected)| (
                        directory.path().join(root),
                        *tier,
                        *availability,
                        *selected
                    ))
                    .collect::<Vec<_>>(),
                "{name}"
            );
        }
    }

    #[test]
    fn missing_package_records_tier_checked() {
        let directory = TempDir::new().unwrap();
        let declared = directory.path().join("declared");
        std::fs::create_dir_all(&declared).unwrap();
        let locations = PackageLocations::default()
            .with_declared_root(declared.clone())
            .unwrap();
        let store = PackageStore::new(locations, PackageFetchPolicy::LocalOnly);

        let failure = store.prepare(&package()).unwrap_err();

        assert_eq!(failure.checks().len(), 1);
        assert_eq!(failure.checks()[0].tier(), PackageTier::Declared);
        assert_eq!(
            failure.checks()[0].candidate(),
            declared.join("preview/demo/0.1.0")
        );
    }

    #[test]
    fn relative_roots_are_rejected() {
        for error in [
            PackageLocations::default()
                .with_declared_root(PathBuf::from("packages"))
                .unwrap_err(),
            PackageLocations::from_absolute_roots(Some(PathBuf::from("relative")), None)
                .unwrap_err(),
        ] {
            assert_eq!(error, FileError::AccessDenied);
        }
    }
}
