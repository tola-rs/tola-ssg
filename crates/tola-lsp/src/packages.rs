//! The packages a query may read: where the site's packages live, and which versions it can
//! import.

use std::path::Path;

use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::diag::EcoString;
use tola_typst::typst::syntax::package::PackageSpec;

use crate::published::PublishedPackage;

pub(super) struct PackageAccess<'a> {
    pub(super) locations: tola_typst::PackageLocations,
    pub(super) boundary: tola_typst::SourceBoundary,
    pub(super) published: &'a [PublishedPackage],
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
        }
    }
}

pub(super) fn installed_versions(
    package_access: &PackageAccess<'_>,
    namespace: &str,
    name: &str,
) -> Vec<tola_typst::typst::syntax::package::PackageVersion> {
    let locations = &package_access.locations;
    let mut versions: Vec<tola_typst::typst::syntax::package::PackageVersion> = locations
        .declared()
        .iter()
        .chain(locations.data())
        .chain(locations.cache())
        .flat_map(|location| installed_packages(location.root(), &package_access.boundary))
        .filter(|(installed, _)| installed.namespace == namespace && installed.name == name)
        .map(|(installed, _)| installed.version)
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
    let locations = &package_access.locations;
    for location in locations
        .declared()
        .iter()
        .chain(locations.data())
        .chain(locations.cache())
    {
        packages.extend(installed_packages(
            location.root(),
            &package_access.boundary,
        ));
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
            let package_path = name.path();
            if boundary.check(&package_path).is_err() || !package_path.is_dir() {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(&package_path) else {
                continue;
            };
            for version in versions.flatten() {
                let version_path = version.path();
                if boundary.check(&version_path).is_err() || !version_path.is_dir() {
                    continue;
                }
                let (namespace, name, version) =
                    (namespace.file_name(), name.file_name(), version.file_name());
                let (Some(namespace), Some(name), Some(version)) =
                    (namespace.to_str(), name.to_str(), version.to_str())
                else {
                    continue;
                };
                let Ok(version) = version.parse() else {
                    continue;
                };
                packages.push((
                    PackageSpec {
                        namespace: namespace.into(),
                        name: name.into(),
                        version,
                    },
                    None,
                ));
            }
        }
    }
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installed package directories read as their namespace, name and version; a directory that
    /// is not one — a missing version, a version that does not parse, a loose file — is left out.
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
