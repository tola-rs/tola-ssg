//! The package observations the engine produces and reads back.

use std::path::Path;
use std::sync::Arc;

use tola_packages::TolaPackage;
use tola_typst::prelude::*;
use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

/// The bytes one package path holds in this compilation, when it holds any.
pub(crate) fn read_package(package: &PackageSpec, path: &str) -> Option<Vec<u8>> {
    let kind = TolaPackage::from_spec(package)?;
    let path = path.strip_prefix('/').unwrap_or(path);
    if kind == TolaPackage::Image
        && let Some(bytes) = tola_packages::observation_bytes(package, path)
    {
        return Some(bytes);
    }
    let path = Path::new(path);
    if is_empty_observation(kind, package, path) {
        return Some(Vec::new());
    }
    kind.file(path).map(|text| text.into_owned().into_bytes())
}

/// These reads record the request in the path itself.
fn is_empty_observation(kind: TolaPackage, package: &PackageSpec, path: &Path) -> bool {
    kind.is_observation_descriptor(path)
        || tola_packages::published_icon_request(package, path).is_some()
}

/// The target one icon package path answers a read with.
pub(crate) fn icon_file_target(
    collections: &tola_icons::IconCollections,
    package: &PackageSpec,
    path: &Path,
) -> tola_typst::FileTarget {
    tola_packages::icon_file_bytes(collections, package, path)
        .map(Arc::<[u8]>::from)
        .or_else(|| {
            tola_packages::icon_collection_names_bytes(collections, package, path).map(Arc::from)
        })
        .map_or(
            tola_typst::FileTarget::Missing,
            tola_typst::FileTarget::Bytes,
        )
}

/// Whether this package path's bytes are fixed for the process.
pub(crate) fn package_file_is_process_stable(package: &PackageSpec, path: &Path) -> bool {
    let Some(kind) = TolaPackage::from_spec(package) else {
        return false;
    };
    is_empty_observation(kind, package, path)
        || kind.file(path).is_some()
        || path
            .to_str()
            .is_some_and(|path| tola_packages::observation_bytes(package, path).is_some())
}

/// The content source a lexical `current-source()` call reads.
pub(crate) fn source_query_file(locator: &ReadLocator) -> Option<RootedPath> {
    let (ReadLocator::Package { package, path } | ReadLocator::ProvidedPackage { package, path }) =
        locator
    else {
        return None;
    };
    let relative = TolaPackage::from_spec(package)?.observed_source_path(path)?;
    let file = VirtualPath::virtualize(Path::new(""), relative).ok()?;
    Some(RootedPath::new(VirtualRoot::Project, file))
}

#[cfg(test)]
mod tests;
