//! Typst packages as `tola.lock` records them.
//!
//! The compiler names the package roots it actually read; this module turns those reads into lock
//! entries, verifies them before a locked build publishes anything, and materializes the vendored
//! copies from the same facts. A key is `@namespace/name`; the value records the version the
//! compiler selected and the digest of that package directory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tola_typst::{
    PackageCheck, PackagePreparationFailure, PackageRead, PackageSpec, PackageStore, PackageTier,
    PackageVerificationError, PackageVerifier,
};

use tola_input::{
    InputLock, InputMode, LockError, LockedPackage, Sha256Digest, TreeHashError, tree_sha256,
};

/// The directory below `vendor/` that holds Typst packages.
pub const PACKAGES_TREE: &str = "typst-packages";

/// One locked package a vendor run materializes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackageRoot {
    name: String,
    version: String,
    relative: PathBuf,
    root: PathBuf,
}

impl LockedPackageRoot {
    /// The lock key: `@namespace/name`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The locked version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The package's destination below `vendor/typst-packages`: `@namespace/name/version`.
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    /// The physical source directory the locked bytes were verified in.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "the package tree of `{name}` at {path} does not match `tola.lock` (lock: {expected}, directory: {actual}); {advice}"
)]
pub struct TreeMismatchError {
    pub name: String,
    pub path: String,
    pub expected: Sha256Digest,
    pub actual: Sha256Digest,
    pub advice: &'static str,
}

/// The failure of recording, verifying, or materializing locked packages.
#[derive(Debug, thiserror::Error)]
pub enum PackageLockError {
    #[error(
        "this build read package `{package}` from {root}, but `tola.lock` records no package `{name}`; run `tola update` to resolve the site's inputs"
    )]
    Unlocked {
        package: String,
        name: String,
        root: String,
    },
    #[error(
        "`tola.lock` records package `{name}` at version {locked}, but this build read version {read}; run `tola update` to resolve the site's inputs"
    )]
    VersionMismatch {
        name: String,
        locked: String,
        read: String,
    },
    #[error(transparent)]
    TreeMismatch(Box<TreeMismatchError>),
    #[error(
        "this build read package `{name}` at two versions, {first} and {second}; a lock records one version per package, so pin one version in the imports"
    )]
    VersionConflict {
        name: String,
        first: String,
        second: String,
    },
    #[error(
        "this build read two different trees for package `{name}` at version {version}, from {first} and {second}; remove one copy, then run `tola update`"
    )]
    TreeConflict {
        name: String,
        version: String,
        first: String,
        second: String,
    },
    #[error("Tola could not hash {path} for package `{name}`")]
    TreeHash {
        name: String,
        path: String,
        #[source]
        source: TreeHashError,
    },
    #[error(
        "`tola.lock` records package `{name}`, but a package key must be `namespace/name`; run `tola update` to rewrite the lock"
    )]
    NameFormat { name: String },
    #[error(
        "`tola.lock` records package `{name}` at version `{version}`, which is not a Typst package version; run `tola update` to rewrite the lock"
    )]
    VersionFormat { name: String, version: String },
    #[error("Tola could not resolve package `{name}` at version {version}: {reason}")]
    Resolve {
        name: String,
        version: String,
        reason: String,
    },
    #[error(
        "`tola.lock` records package `{name}`, but {path} is missing; run `tola vendor` to materialize the locked packages"
    )]
    VendorMissing { name: String, path: String },
    #[error(
        "{path} holds `{found}`, which `tola.lock` does not record; remove it, or run `tola update` if the site really reads it"
    )]
    VendorExtra { path: String, found: String },
    #[error(
        "{path} is not a package directory; run `tola vendor` to replace it with the locked packages"
    )]
    VendorNotDirectory { path: String },
    #[error("Tola could not read {path}")]
    VendorRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Lock(#[from] LockError),
}

/// Record every package root one build read as lock entries.
///
/// A lock key names one package, so reading two versions of the same package is an error rather
/// than a silent choice, and two different trees at one version are an error rather than a coin
/// flip.
pub fn record_package_reads(
    reads: &[PackageRead],
) -> Result<BTreeMap<String, LockedPackage>, PackageLockError> {
    let mut packages = BTreeMap::new();
    for read in reads {
        let name = package_key(read.package());
        let version = read.version().to_string();
        let path = describe_root(read.root(), Some(read.tier()), None);
        let digest = tree_sha256(read.root()).map_err(|source| PackageLockError::TreeHash {
            name: name.clone(),
            path: path.clone(),
            source,
        })?;
        match packages.get(&name) {
            None => {
                packages.insert(
                    name,
                    (
                        LockedPackage {
                            version,
                            tree_sha256: digest,
                        },
                        path,
                    ),
                );
            }
            Some((locked, recorded)) => {
                if locked.version != version {
                    return Err(PackageLockError::VersionConflict {
                        name,
                        first: locked.version.clone(),
                        second: version,
                    });
                }
                if locked.tree_sha256 != digest {
                    return Err(PackageLockError::TreeConflict {
                        name,
                        version,
                        first: recorded.clone(),
                        second: path,
                    });
                }
            }
        }
    }
    Ok(packages
        .into_iter()
        .map(|(name, (locked, _))| (name, locked))
        .collect())
}

/// Record one build's package reads in the lock, when this attempt may write it.
///
/// The lock is read first so the module and icon sections survive; only the package table is
/// replaced. An unchanged table writes nothing, so a build does not churn the file. Returns whether
/// the lock changed.
pub fn write_package_lock(
    site_root: &Path,
    reads: &[PackageRead],
    mode: InputMode,
) -> Result<bool, PackageLockError> {
    if !mode.may_write_lock() {
        return Ok(false);
    }
    let mut lock = InputLock::read(site_root)?.unwrap_or_default();
    let packages = record_package_reads(reads)?;
    if lock.packages == packages {
        return Ok(false);
    }
    lock.packages = packages;
    lock.write_atomic(site_root)?;
    Ok(true)
}

/// Verify every package root this build read against the lock.
///
/// Existing package versions must retain their locked bytes. A build that may write the lock
/// can introduce a new explicitly imported version, but cannot silently replace a known one.
pub fn verify_package_reads(
    reads: &[PackageRead],
    lock: &InputLock,
    mode: InputMode,
    site_root: &Path,
) -> Result<(), PackageLockError> {
    for read in reads {
        if !must_verify(lock, mode, read.package()) {
            continue;
        }
        verify_package_root(
            read.package(),
            read.root(),
            Some(read.tier()),
            lock,
            site_root,
        )?;
    }
    Ok(())
}

/// The pre-read check of every package root whose version is already locked.
///
/// Install it on the resolver's package store so an unverified directory can never supply bytes to
/// a compilation. New versions may be recorded when the invocation allows lock writes.
pub fn verifier(lock: &InputLock, mode: InputMode, site_root: &Path) -> Option<PackageVerifier> {
    if !mode.requires_lock() && lock.packages.is_empty() {
        return None;
    }
    let lock = lock.clone();
    let site_root = site_root.to_path_buf();
    Some(std::sync::Arc::new(
        move |package: &PackageSpec, root: &Path| {
            if !must_verify(&lock, mode, package) {
                return Ok(());
            }
            verify_package_root(package, root, None, &lock, &site_root)
                .map_err(|error| PackageVerificationError::new(error.to_string()))
        },
    ))
}

fn must_verify(lock: &InputLock, mode: InputMode, package: &PackageSpec) -> bool {
    mode.requires_lock()
        || lock
            .packages
            .get(&package_key(package))
            .is_some_and(|locked| locked.version == package.version.to_string())
}

/// Resolve every locked package from the package roots this site reads, verifying the locked tree.
///
/// This is what `tola vendor` materializes: the lock names the packages, and the existing vendored
/// copy or a host root supplies the bytes. A source that does not match the lock fails instead of
/// being copied.
pub fn resolve_locked_packages(
    store: &PackageStore,
    lock: &InputLock,
    site_root: &Path,
) -> Result<Vec<LockedPackageRoot>, PackageLockError> {
    let mut packages = Vec::with_capacity(lock.packages.len());
    for (name, locked) in &lock.packages {
        let (namespace, package_name) = split_name(name)?;
        let spec = format!("{namespace}/{package_name}:{}", locked.version)
            .parse::<PackageSpec>()
            .map_err(|_| PackageLockError::VersionFormat {
                name: name.clone(),
                version: locked.version.clone(),
            })?;
        let prepared = store
            .prepare(&spec)
            .map_err(|failure| resolve_failure(name, locked, failure))?;
        let root = prepared.directory().to_path_buf();
        let tier = prepared
            .checks()
            .iter()
            .find(|check| check.was_selected())
            .map(PackageCheck::tier);
        let actual = tree_sha256(&root).map_err(|source| PackageLockError::TreeHash {
            name: name.clone(),
            path: describe_root(&root, tier, Some(site_root)),
            source,
        })?;
        if actual != locked.tree_sha256 {
            return Err(PackageLockError::TreeMismatch(Box::new(
                TreeMismatchError {
                    name: name.clone(),
                    path: describe_root(&root, tier, Some(site_root)),
                    expected: locked.tree_sha256,
                    actual,
                    advice: "run `tola update` to resolve the package again",
                },
            )));
        }
        packages.push(LockedPackageRoot {
            name: name.clone(),
            version: locked.version.clone(),
            relative: PathBuf::from(namespace)
                .join(package_name)
                .join(&locked.version),
            root,
        });
    }
    Ok(packages)
}

/// Verify that the vendored packages are exactly the locked set, with the locked trees.
///
/// A missing package, an extra directory, and changed bytes are all failures: a pure build reads
/// `vendor/` as the complete package input, so a tree that disagrees with the lock cannot build.
pub fn verify_vendored_packages(
    vendor_root: &Path,
    lock: &InputLock,
    site_root: &Path,
) -> Result<(), PackageLockError> {
    let packages_root = vendor_root.join(PACKAGES_TREE);
    let mut expected = BTreeSet::new();
    for (name, locked) in &lock.packages {
        let (namespace, package_name) = split_name(name)?;
        let relative = PathBuf::from(namespace)
            .join(package_name)
            .join(&locked.version);
        expected.insert(relative.clone());
        verify_vendored_root(name, &packages_root.join(&relative), locked, site_root)?;
    }
    verify_no_extra_packages(&packages_root, &expected, site_root)
}

/// The lock key of one package: `@namespace/name`.
fn package_key(package: &PackageSpec) -> String {
    format!("@{}/{}", package.namespace, package.name)
}

/// Split a lock key into its namespace and name.
fn split_name(name: &str) -> Result<(&str, &str), PackageLockError> {
    match name.split_once('/') {
        Some((namespace, package))
            if !namespace.is_empty() && !package.is_empty() && !package.contains('/') =>
        {
            Ok((namespace, package))
        }
        _ => Err(PackageLockError::NameFormat {
            name: name.to_owned(),
        }),
    }
}

/// Verify one package root against the lock.
fn verify_package_root(
    package: &PackageSpec,
    root: &Path,
    tier: Option<PackageTier>,
    lock: &InputLock,
    site_root: &Path,
) -> Result<(), PackageLockError> {
    let name = package_key(package);
    let Some(locked) = lock.packages.get(&name) else {
        return Err(PackageLockError::Unlocked {
            package: package.to_string(),
            name,
            root: describe_root(root, tier, Some(site_root)),
        });
    };
    let version = package.version.to_string();
    if locked.version != version {
        return Err(PackageLockError::VersionMismatch {
            name,
            locked: locked.version.clone(),
            read: version,
        });
    }
    let path = describe_root(root, tier, Some(site_root));
    let actual = tree_sha256(root).map_err(|source| PackageLockError::TreeHash {
        name: name.clone(),
        path: path.clone(),
        source,
    })?;
    if actual != locked.tree_sha256 {
        return Err(PackageLockError::TreeMismatch(Box::new(
            TreeMismatchError {
                name,
                path,
                expected: locked.tree_sha256,
                actual,
                advice: repair_advice(root, site_root),
            },
        )));
    }
    Ok(())
}

/// Verify one vendored package root against its lock entry.
fn verify_vendored_root(
    name: &str,
    root: &Path,
    locked: &LockedPackage,
    site_root: &Path,
) -> Result<(), PackageLockError> {
    let path = describe_root(root, None, Some(site_root));
    let metadata = match std::fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PackageLockError::VendorMissing {
                name: name.to_owned(),
                path,
            });
        }
        Err(source) => {
            return Err(PackageLockError::VendorRead { path, source });
        }
    };
    if !metadata.is_dir() {
        return Err(PackageLockError::VendorNotDirectory { path });
    }
    let actual = tree_sha256(root).map_err(|source| PackageLockError::TreeHash {
        name: name.to_owned(),
        path: path.clone(),
        source,
    })?;
    if actual != locked.tree_sha256 {
        return Err(PackageLockError::TreeMismatch(Box::new(
            TreeMismatchError {
                name: name.to_owned(),
                path,
                expected: locked.tree_sha256,
                actual,
                advice: "run `tola vendor` to restore the vendored copy",
            },
        )));
    }
    Ok(())
}

/// Refuse every directory below `typst-packages` that no lock entry names.
fn verify_no_extra_packages(
    packages_root: &Path,
    expected: &BTreeSet<PathBuf>,
    site_root: &Path,
) -> Result<(), PackageLockError> {
    let namespaces = match std::fs::read_dir(packages_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_empty() => {
            return Ok(());
        }
        Err(source) => {
            return Err(PackageLockError::VendorRead {
                path: describe_root(packages_root, None, Some(site_root)),
                source,
            });
        }
    };
    for namespace in namespaces {
        let namespace = namespace.map_err(|source| PackageLockError::VendorRead {
            path: describe_root(packages_root, None, Some(site_root)),
            source,
        })?;
        let namespace_path = namespace.path();
        if !namespace.file_type().is_ok_and(|kind| kind.is_dir()) {
            return Err(extra_package(&namespace_path, packages_root, site_root));
        }
        let names =
            std::fs::read_dir(&namespace_path).map_err(|source| PackageLockError::VendorRead {
                path: describe_root(&namespace_path, None, Some(site_root)),
                source,
            })?;
        for name in names {
            let name = name.map_err(|source| PackageLockError::VendorRead {
                path: describe_root(&namespace_path, None, Some(site_root)),
                source,
            })?;
            let name_path = name.path();
            if !name.file_type().is_ok_and(|kind| kind.is_dir()) {
                return Err(extra_package(&name_path, packages_root, site_root));
            }
            let versions =
                std::fs::read_dir(&name_path).map_err(|source| PackageLockError::VendorRead {
                    path: describe_root(&name_path, None, Some(site_root)),
                    source,
                })?;
            for version in versions {
                let version = version.map_err(|source| PackageLockError::VendorRead {
                    path: describe_root(&name_path, None, Some(site_root)),
                    source,
                })?;
                let version_path = version.path();
                let relative = version_path
                    .strip_prefix(packages_root)
                    .unwrap_or(&version_path)
                    .to_path_buf();
                let recorded = version.file_type().is_ok_and(|kind| kind.is_dir())
                    && expected.contains(&relative);
                if !recorded {
                    return Err(extra_package(&version_path, packages_root, site_root));
                }
            }
        }
    }
    Ok(())
}

/// The refusal of one directory the lock does not name.
///
/// `found` is the path below `typst-packages` that no lock entry names.
fn extra_package(path: &Path, packages_root: &Path, site_root: &Path) -> PackageLockError {
    let found = path.strip_prefix(packages_root).unwrap_or(path);
    PackageLockError::VendorExtra {
        path: describe_root(path, None, Some(site_root)),
        found: found.display().to_string(),
    }
}

/// Name one package root for a site author: the site-relative path when the site holds it, else
/// the storage tier that supplied it. A host directory never reaches a message.
fn describe_root(root: &Path, tier: Option<PackageTier>, site_root: Option<&Path>) -> String {
    let site_relative = site_root.filter(|site_root| site_holds(root, site_root));
    if let Some(site_root) = site_relative {
        return format!("`{}`", tola_diagnostic::display_path(root, site_root));
    }
    match tier {
        Some(PackageTier::Declared) => "the declared package root".to_owned(),
        Some(PackageTier::Data) => "the local package directory".to_owned(),
        Some(PackageTier::Cache) => "the Typst package cache".to_owned(),
        None => "the package directory".to_owned(),
    }
}

/// Whether a package root is inside the site, including when the two are spelled differently.
fn site_holds(root: &Path, site_root: &Path) -> bool {
    root.starts_with(site_root)
        || tola_input::normalize_existing_prefix(root)
            .starts_with(tola_input::normalize_existing_prefix(site_root))
}

/// The next step for a tree that disagrees with the lock.
fn repair_advice(root: &Path, site_root: &Path) -> &'static str {
    if site_holds(root, site_root) {
        "run `tola vendor` to restore the vendored copy"
    } else {
        "run `tola update` to resolve the package again"
    }
}

/// The author-facing reason a locked package could not be resolved.
fn resolve_failure(
    name: &str,
    locked: &LockedPackage,
    failure: PackagePreparationFailure,
) -> PackageLockError {
    use tola_typst::typst::diag::{FileError, PackageError};
    let (error, _) = failure.into_error_and_checks();
    let reason = match &error {
        FileError::Package(PackageError::NotFound(_)) => {
            "no installed copy was found and downloads are unavailable".to_owned()
        }
        _ => error.to_string(),
    };
    PackageLockError::Resolve {
        name: name.to_owned(),
        version: locked.version.clone(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use tola_input::InputScope;

    fn spec(name: &str, version: &str) -> PackageSpec {
        format!("@{name}:{version}").parse().unwrap()
    }

    fn package_tree(root: &Path, name: &str, version: &str) -> PathBuf {
        let directory = root.join(name).join(version);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("lib.typ"), format!("= {name} {version}")).unwrap();
        directory
    }

    fn locked(version: &str, root: &Path) -> LockedPackage {
        LockedPackage {
            version: version.to_owned(),
            tree_sha256: tree_sha256(root).unwrap(),
        }
    }

    fn mode(scope: InputScope, locked: bool) -> InputMode {
        InputMode::new(scope, locked)
    }

    #[test]
    fn verifier_refuses_an_unlocked_or_changed_package() {
        let site = TempDir::new().unwrap();
        let root = package_tree(site.path(), "preview/demo", "0.1.0");
        let mut lock = InputLock::default();
        lock.packages
            .insert("@preview/demo".to_owned(), locked("0.1.0", &root));

        let online = verifier(&lock, mode(InputScope::Online, false), site.path()).unwrap();
        let check = verifier(&lock, mode(InputScope::Pure, false), site.path()).unwrap();

        assert!(check(&spec("preview/demo", "0.1.0"), &root).is_ok());
        assert!(online(&spec("preview/demo", "0.1.0"), &root).is_ok());
        assert!(check(&spec("preview/other", "0.1.0"), &root).is_err());

        std::fs::write(root.join("lib.typ"), "= changed").unwrap();
        assert!(check(&spec("preview/demo", "0.1.0"), &root).is_err());
        assert!(online(&spec("preview/demo", "0.1.0"), &root).is_err());
    }

    #[test]
    fn vendored_tree_must_match_the_lock_exactly() {
        let site = TempDir::new().unwrap();
        let vendor = site.path().join("vendor");
        let vendored = vendor.join("typst-packages/@preview/demo/0.1.0");
        std::fs::create_dir_all(&vendored).unwrap();
        std::fs::write(vendored.join("lib.typ"), "= demo").unwrap();
        let mut lock = InputLock::default();
        lock.packages
            .insert("@preview/demo".to_owned(), locked("0.1.0", &vendored));

        verify_vendored_packages(&vendor, &lock, site.path()).unwrap();

        let extra = vendor.join("typst-packages/preview/extra/1.0.0");
        std::fs::create_dir_all(&extra).unwrap();
        let error = verify_vendored_packages(&vendor, &lock, site.path()).unwrap_err();
        assert!(
            matches!(error, PackageLockError::VendorExtra { .. }),
            "{error}"
        );
        std::fs::remove_dir_all(&extra).unwrap();

        std::fs::write(vendored.join("lib.typ"), "= changed").unwrap();
        let error = verify_vendored_packages(&vendor, &lock, site.path()).unwrap_err();
        assert!(
            matches!(error, PackageLockError::TreeMismatch(_)),
            "{error}"
        );
        std::fs::write(vendored.join("lib.typ"), "= demo").unwrap();

        std::fs::remove_dir_all(&vendored).unwrap();
        let error = verify_vendored_packages(&vendor, &lock, site.path()).unwrap_err();
        assert!(
            matches!(error, PackageLockError::VendorMissing { .. }),
            "{error}"
        );
    }
}
