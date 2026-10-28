//! The files a source URI names: site paths, builtin package files, and Tola's own packages.
//!
//! An editor addresses a file by URI, a compiler by [`FileId`], and the site by a path; these
//! conversions are the only place those three spellings meet.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;

use anyhow::Result;
use lsp_types::Uri;
use tola_packages::{BuiltinPackage, TolaPackage, builtin_package, resolvable_packages};
use tola_typst::typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use url::Url;

/// The editor URI that names no source this server serves.
///
/// The message is what a request failure carries; the URI itself stays with the caller, which is
/// the only layer that knows which document the address came from.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceUriError {
    /// A scheme or shape this server publishes no documents under.
    #[error("unsupported source URI")]
    Unsupported,
    /// A `tola-package:` address outside the registry this implementation ships.
    #[error("unknown embedded package source")]
    UnknownEmbedded,
    /// A site path outside the source root.
    #[error("source is outside the site root")]
    OutsideSite,
    /// A site address this server could not spell as a protocol URI.
    #[error("source URL is not a valid protocol URI")]
    Protocol(#[source] crate::uri::UriError),
}

pub(crate) fn path_id(path: &Path, root: &Path) -> Option<FileId> {
    let logical = tola_build::filesystem::lexical_path_identity(path);
    let physical = tola_build::filesystem::normalize_existing_prefix(&logical);
    tola_typst::file_id_from_path(&physical, root)
}

pub(crate) fn file_id(uri: &Uri, root: &Path) -> Result<FileId> {
    let url = Url::parse(uri.as_str())?;
    if url.scheme() != "file" {
        return Ok(package_url_id(&url)?);
    }
    let path = crate::uri::site_path_of(&url)?;
    let Some(id) = path_id(&path, root) else {
        return Err(SourceUriError::OutsideSite.into());
    };
    Ok(id)
}

/// A verified mirror keeps its embedded package identity wherever the client stores it.
pub(crate) fn source_id(
    uri: &Uri,
    root: &Path,
    package_sources: Option<&Path>,
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<FileId>> {
    source_id_with_packages(uri, root, package_sources, boundary, &Arc::default(), None)
}

/// Address a selected local module before treating a file as a project or global package source.
/// Several aliases of one physical file need an explicit context; none is guessed here.
pub(crate) fn source_id_with_packages(
    uri: &Uri,
    root: &Path,
    package_sources: Option<&Path>,
    boundary: &tola_typst::SourceBoundary,
    local_packages: &Arc<tola_typst::LocalPackages>,
    project_sources: Option<(&Path, &[PathBuf])>,
) -> Result<Option<FileId>> {
    let Ok(url) = Url::parse(uri.as_str()) else {
        return Ok(None);
    };
    if url.scheme() != "file" {
        return Ok(package_url_id(&url).ok());
    }
    let Ok(path) = crate::uri::site_path_of(&url) else {
        return Ok(None);
    };
    let mirror = tola_build::filesystem::layout::builtin_packages(root);
    for directory in package_sources.into_iter().chain([mirror.as_path()]) {
        let directory = tola_build::filesystem::normalize_existing_prefix(directory);
        if path.starts_with(&directory) {
            return Ok(package_view_id(&path, &directory));
        }
    }
    if boundary.refusal(&path)?.is_some() {
        return Ok(None);
    }
    if project_sources
        .is_some_and(|(content, files)| path.starts_with(content) || files.contains(&path))
    {
        return Ok(tola_typst::file_id_from_path(&path, root));
    }
    let resolver = tola_typst::FileResolver::new()
        .with_source_boundary(boundary.clone())
        .with_local_packages(Arc::clone(local_packages));
    let spelled = crate::uri::file_path_of(&url)?;
    match resolver.local_package_ids(&spelled)?.as_slice() {
        [id] => return Ok(Some(*id)),
        [] => {}
        _ => return Ok(None),
    }
    let id = tola_typst::file_id_from_path(&path, root)
        .or_else(|| tola_build::filesystem::package_document_id(&path, None));
    Ok(id.filter(|id| {
        !matches!(id.root(), VirtualRoot::Package(package)
        if local_packages.owns_namespace(&package.namespace))
    }))
}

pub(crate) fn package_file_id(uri: &Uri) -> Result<FileId> {
    Ok(package_url_id(&Url::parse(uri.as_str())?)?)
}

fn package_url_id(url: &Url) -> Result<FileId, SourceUriError> {
    if url.scheme() != "tola-package"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(SourceUriError::Unsupported);
    }
    PACKAGE_FILE_ADDRESSES
        .get(url.as_str())
        .copied()
        .ok_or(SourceUriError::UnknownEmbedded)
}

/// The identity this implementation reads one builtin package file at, when `path` spells one.
fn builtin_file_id(package: BuiltinPackage, path: &str) -> Option<FileId> {
    let vpath = VirtualPath::new(path).ok()?;
    Some(RootedPath::new(VirtualRoot::Package(package.spec()), vpath).intern())
}

/// The address this implementation publishes for every builtin package file, by that address.
///
/// Matching builtin registry identities rather than an untrusted filesystem path means comparing
/// against the addresses this server itself writes; building them once keeps a lookup from
/// re-deriving each one.
static PACKAGE_FILE_ADDRESSES: LazyLock<HashMap<String, FileId>> = LazyLock::new(|| {
    let mut addresses = HashMap::new();
    for package in resolvable_packages() {
        for (path, _) in package.files() {
            let Some(id) = builtin_file_id(package, path) else {
                continue;
            };
            if let Ok(uri) = source_uri(id, Path::new("")) {
                addresses.insert(uri.as_str().to_owned(), id);
            }
        }
    }
    addresses
});

pub(crate) fn source_uri(id: FileId, root: &Path) -> Result<Uri, SourceUriError> {
    let path = id.vpath().get_without_slash();
    if let VirtualRoot::Package(package) = id.root() {
        let mut uri = Url::parse("tola-package:/")
            .map_err(|error| SourceUriError::Protocol(crate::uri::UriError::Invalid(error)))?;
        uri.path_segments_mut()
            .expect("a package URI has path segments")
            .pop_if_empty()
            .push(&package.namespace)
            .push(&package.name)
            .push(&package.version.to_string())
            .extend(path.split('/'));
        crate::uri::from_url(uri).map_err(SourceUriError::Protocol)
    } else {
        crate::uri::from_file_path(&root.join(path)).map_err(SourceUriError::Protocol)
    }
}

/// The address one file answers under, in the spelling the client named its root with.
///
/// File identities resolve from the source root; the client root chooses only the URI spelling.
pub(crate) fn client_uri(
    id: FileId,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
) -> Result<Uri, SourceUriError> {
    if matches!(id.root(), VirtualRoot::Project) {
        return client_root
            .address(&source_root.join(id.vpath().get_without_slash()))
            .map_err(SourceUriError::Protocol);
    }
    source_uri(id, source_root)
}

pub(crate) fn embedded_source(id: FileId) -> Option<Cow<'static, str>> {
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    builtin_package(spec)?
        .files()
        .find(|(path, _)| *path == id.vpath().get_without_slash())
        .map(|(_, text)| text)
}

/// The file one builtin package source is read at in the editor's package view.
///
/// A file-only client cannot open a `tola-package:` document, so it reads a site's packages as the
/// mirrors `tola editor setup` publishes beside it, and a definition into a package names one of
/// them.
pub(crate) fn package_view_path(id: FileId, directory: &Path) -> Option<PathBuf> {
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    Some(
        directory
            .join(spec.namespace.as_str())
            .join(spec.name.as_str())
            .join(spec.version.to_string())
            .join(id.vpath().get_without_slash()),
    )
}

/// Whether the file an editor reads has exactly the package source it mirrors.
///
/// A mirror some other Tola wrote holds positions the builtin source does not, so the bytes decide
/// whether the file is this implementation's package source at all.
pub(crate) fn verify_package_view(path: &Path, source: &str) -> Result<(), PackageViewError> {
    let file = std::fs::File::open(path).map_err(PackageViewError::Unreadable)?;
    let mut contents = Vec::with_capacity(source.len());
    file.take(source.len() as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(PackageViewError::Unreadable)?;
    if contents != source.as_bytes() {
        return Err(PackageViewError::Differs);
    }
    Ok(())
}

/// Why a mirror file is not the builtin package source it mirrors.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PackageViewError {
    #[error("Tola could not open an editor package source; run `tola editor setup`")]
    Unreadable(#[source] std::io::Error),
    #[error(
        "an editor package source differs from this Tola implementation; run `tola editor setup`"
    )]
    Differs,
}

fn package_view_id(path: &Path, directory: &Path) -> Option<FileId> {
    for package in resolvable_packages() {
        for (file, source) in package.files() {
            let Some(id) = builtin_file_id(package, file) else {
                continue;
            };
            if package_view_path(id, directory).as_deref() != Some(path) {
                continue;
            }
            return verify_package_view(path, &source).is_ok().then_some(id);
        }
    }
    None
}

/// Where one package document's bytes are read from, among the locations the compiler resolved.
///
/// A builtin source is embedded, every other package — `@preview`, a local package, a vendored one —
/// is a file below a package location. The order follows the compiler's: declared, data, then cache.
pub(crate) fn package_file_path(
    id: FileId,
    locations: &tola_typst::PackageLocations,
    boundary: &tola_typst::SourceBoundary,
) -> Option<PathBuf> {
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    let package =
        tola_typst::PackageStore::new(locations.clone(), tola_typst::PackageFetchPolicy::LocalOnly)
            .with_source_boundary(boundary.clone())
            .prepare(spec)
            .ok()?;
    Some(package.directory().join(id.vpath().get_without_slash()))
}

/// Prefer the request's selected local package over global package directories.
/// This maps unsaved buffers as well as disk bytes, and never exposes virtual manifests.
pub(crate) fn package_file_path_with_packages(
    id: FileId,
    locations: &tola_typst::PackageLocations,
    boundary: &tola_typst::SourceBoundary,
    local_packages: &Arc<tola_typst::LocalPackages>,
) -> Option<PathBuf> {
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    if local_packages.package(spec).is_some() || local_packages.owns_namespace(&spec.namespace) {
        return tola_typst::FileResolver::new()
            .with_source_boundary(boundary.clone())
            .with_local_packages(Arc::clone(local_packages))
            .local_package_path(id)
            .ok()
            .flatten();
    }
    package_file_path(id, locations, boundary)
}

/// The builtin package file a path inside one builtin package names, when this implementation
/// ships it.
pub(crate) fn embedded_relative(id: FileId, path: &str) -> Option<FileId> {
    embedded_source(id)?;
    let target = tola_typst::typst::foundations::PathOrStr::Str(path.into())
        .resolve(id)
        .ok()?
        .intern();
    embedded_source(target).map(|_| target)
}

pub(crate) fn is_tola_span(span: tola_typst::typst::syntax::Span) -> bool {
    span.id().is_some_and(|id| {
        matches!(id.root(), VirtualRoot::Package(spec) if TolaPackage::from_spec(spec).is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_files_share_selected_root() {
        let directory = tempfile::tempdir().unwrap();
        let declared = directory.path().join("declared");
        let cache = directory.path().join("cache");
        let package = "local/example/1.0.0";
        for root in [&declared, &cache] {
            std::fs::create_dir_all(root.join(package)).unwrap();
        }
        std::fs::write(cache.join(package).join("extra.typ"), "#let extra = 1").unwrap();
        let locations = tola_typst::PackageLocations::from_absolute_roots(None, Some(cache))
            .unwrap()
            .with_declared_root(declared.clone())
            .unwrap();
        let id = RootedPath::new(
            VirtualRoot::Package("@local/example:1.0.0".parse().unwrap()),
            VirtualPath::new("extra.typ").unwrap(),
        )
        .intern();
        let path =
            package_file_path(id, &locations, &tola_typst::SourceBoundary::default()).unwrap();

        assert_eq!(
            path,
            declared
                .canonicalize()
                .unwrap()
                .join(package)
                .join("extra.typ")
        );
        assert!(!path.exists());
    }
}
