//! The packages a query may read: where the site's packages live, and which versions it can
//! import.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::diag::EcoString;
use tola_typst::typst::syntax::package::{PackageSpec, PackageVersion};

use crate::published::PublishedPackage;

pub(super) struct PackageAccess<'a> {
    pub(super) locations: tola_typst::PackageLocations,
    pub(super) boundary: tola_typst::SourceBoundary,
    pub(super) published: &'a [PublishedPackage],
    pub(super) local_packages: Arc<tola_typst::LocalPackages>,
    pub(super) project_content: PathBuf,
    pub(super) project_files: Vec<PathBuf>,
}

impl<'a> PackageAccess<'a> {
    pub(super) fn new(
        config: &ResolvedSiteConfig,
        resources: &tola_build::BuildResources,
        published: &'a [PublishedPackage],
    ) -> Self {
        Self {
            locations: resources.package_locations(config),
            boundary: resources.source_boundary(config),
            published: if resources.network_access() == tola_build::NetworkAccess::Allowed {
                published
            } else {
                &[]
            },
            local_packages: Arc::clone(config.local_packages()),
            project_content: tola_build::filesystem::normalize_existing_prefix(
                &config.build().content_dir,
            ),
            project_files: config
                .project_entry_path()
                .into_iter()
                .chain(
                    config
                        .modules()
                        .values()
                        .flat_map(|module| module.overrides.values())
                        .map(|path| config.get_root().join(path)),
                )
                .map(|path| tola_build::filesystem::normalize_existing_prefix(&path))
                .collect(),
        }
    }

    /// Static package admission for one request; compiled worlds retain their own selection.
    pub(super) fn prepared(
        config: &ResolvedSiteConfig,
        resources: &tola_build::BuildResources,
        published: &'a [PublishedPackage],
    ) -> anyhow::Result<Self> {
        let prepared = tola_build::modules::prepare(config, resources.input_scope())?;
        Ok(Self::new(&prepared, resources, published))
    }
}

pub(super) fn installed_versions(
    package_access: &PackageAccess<'_>,
    namespace: &str,
    name: &str,
) -> Vec<PackageVersion> {
    if [namespace, name]
        .iter()
        .any(|part| Path::new(part).file_name() != Some(std::ffi::OsStr::new(part)))
    {
        return Vec::new();
    }
    if package_access.local_packages.owns_namespace(namespace) {
        let mut versions = package_access
            .local_packages
            .iter()
            .filter(|package| package.spec().namespace == namespace && package.spec().name == name)
            .map(|package| package.spec().version)
            .collect::<Vec<_>>();
        versions.sort();
        versions.dedup();
        return versions;
    }
    let locations = &package_access.locations;
    let mut versions: Vec<PackageVersion> = locations
        .declared()
        .iter()
        .chain(locations.data())
        .chain(locations.cache())
        .flat_map(|location| {
            package_versions(
                &location.root().join(namespace).join(name),
                &package_access.boundary,
            )
        })
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

pub(super) fn importable_packages(
    package_access: &PackageAccess<'_>,
) -> Vec<(PackageSpec, Option<EcoString>)> {
    let mut packages: Vec<(PackageSpec, Option<EcoString>)> = tola_packages::builtin_packages()
        .map(|package| (package.spec(), None))
        .collect();
    packages.extend(
        package_access
            .local_packages
            .iter()
            .map(|package| (package.spec().clone(), None)),
    );
    let locations = &package_access.locations;
    for location in locations
        .declared()
        .iter()
        .chain(locations.data())
        .chain(locations.cache())
    {
        packages.extend(
            installed_packages(location.root(), &package_access.boundary)
                .into_iter()
                .filter(|(spec, _)| {
                    !package_access
                        .local_packages
                        .owns_namespace(&spec.namespace)
                }),
        );
    }
    packages.extend(package_access.published.iter().filter_map(|published| {
        let version = published.version.parse().ok()?;
        Some((
            PackageSpec {
                namespace: "preview".into(),
                name: published.name.as_str().into(),
                version,
            },
            published.description.as_deref().map(EcoString::from),
        ))
    }));
    // A name position offers one entry per package — the newest version the site can obtain — so an
    // author reads package names, not every published version of each. A version position lists
    // every version instead, which `version_completions` answers.
    packages.sort_by(|left, right| {
        (
            &left.0.namespace,
            &left.0.name,
            std::cmp::Reverse(&left.0.version),
        )
            .cmp(&(
                &right.0.namespace,
                &right.0.name,
                std::cmp::Reverse(&right.0.version),
            ))
    });
    packages.dedup_by(|left, right| {
        left.0.namespace == right.0.namespace && left.0.name == right.0.name
    });
    packages
}

/// The packages installed below one root, in the `namespace/name/version` layout.
fn installed_packages(
    root: &Path,
    boundary: &tola_typst::SourceBoundary,
) -> Vec<(PackageSpec, Option<EcoString>)> {
    let mut packages = Vec::new();
    if boundary.check(root).is_err() {
        return packages;
    }
    let Ok(namespaces) = std::fs::read_dir(root) else {
        return packages;
    };
    for namespace in namespaces.flatten() {
        let namespace_path = namespace.path();
        if boundary.check(&namespace_path).is_err() || !namespace_path.is_dir() {
            continue;
        }
        let Ok(names) = std::fs::read_dir(&namespace_path) else {
            continue;
        };
        for name in names.flatten() {
            let (namespace, package) = (namespace.file_name(), name.file_name());
            let (Some(namespace), Some(package)) = (namespace.to_str(), package.to_str()) else {
                continue;
            };
            for version in package_versions(&name.path(), boundary) {
                packages.push((
                    PackageSpec {
                        namespace: namespace.into(),
                        name: package.into(),
                        version,
                    },
                    None,
                ));
            }
        }
    }
    packages
}

fn package_versions(root: &Path, boundary: &tola_typst::SourceBoundary) -> Vec<PackageVersion> {
    if boundary.check(root).is_err() {
        return Vec::new();
    }
    let Ok(versions) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    versions
        .flatten()
        .filter_map(|version| {
            let number = version.file_name().to_str()?.parse().ok()?;
            let path = version.path();
            (boundary.check(&path).is_ok() && path.is_dir()).then_some(number)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_versions_merge_search_roots() {
        let directory = tempfile::tempdir().unwrap();
        let declared = directory.path().join("declared");
        let cache = directory.path().join("cache");
        for (root, versions) in [
            (&declared, &["0.1.0", "1.0.0"][..]),
            (&cache, &["1.0.0", "2.0.0", "unfinished"][..]),
        ] {
            for version in versions {
                std::fs::create_dir_all(root.join("local/example").join(version)).unwrap();
            }
        }
        std::fs::write(cache.join("local/example/3.0.0"), "not a directory").unwrap();
        let access = PackageAccess {
            locations: tola_typst::PackageLocations::from_absolute_roots(None, Some(cache))
                .unwrap()
                .with_declared_root(declared)
                .unwrap(),
            boundary: tola_typst::SourceBoundary::new(directory.path(), true),
            published: &[],
            local_packages: Arc::default(),
            project_content: directory.path().join("content"),
            project_files: Vec::new(),
        };

        assert_eq!(
            installed_versions(&access, "local", "example"),
            ["0.1.0", "1.0.0", "2.0.0"].map(|version| version.parse().unwrap())
        );
        for (namespace, name) in [
            ("local", "missing"),
            ("local", "../local/example"),
            ("local", "example/"),
            ("", "example"),
        ] {
            assert!(installed_versions(&access, namespace, name).is_empty());
        }
    }

    #[test]
    fn installed_packages_read_their_specs() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("local/example/0.1.0")).unwrap();
        std::fs::create_dir_all(directory.path().join("local/example")).unwrap();
        std::fs::create_dir_all(directory.path().join("local/other/not-a-version")).unwrap();
        std::fs::create_dir_all(directory.path().join("local/fine/1.2.3")).unwrap();
        std::fs::write(directory.path().join("local/loose-file"), "").unwrap();
        let mut specs: Vec<String> = installed_packages(
            directory.path(),
            &tola_typst::SourceBoundary::new(directory.path(), true),
        )
        .into_iter()
        .map(|(spec, _)| spec.to_string())
        .collect();
        specs.sort();
        assert_eq!(specs, ["@local/example:0.1.0", "@local/fine:1.2.3"]);
    }
}
