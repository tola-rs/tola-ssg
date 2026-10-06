//! The files a source URI names: site paths, builtin package files, and Tola's own packages.
//!
//! An editor addresses a file by URI, a compiler by [`FileId`], and the site by a path; these
//! conversions are the only place those three spellings meet.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use lsp_types::Uri;
use tola_packages::{BuiltinPackage, TolaPackage, builtin_package, resolvable_packages};
use tola_typst::typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
use url::Url;

pub(crate) fn path_id(path: &Path, root: &Path) -> Option<FileId> {
    let logical = tola_build::filesystem::lexical_path_identity(path);
    let physical = tola_build::filesystem::normalize_existing_prefix(&logical);
    tola_typst::file_id_from_path(&physical, root)
}

pub(crate) fn file_id(uri: &Uri, root: &Path) -> Result<FileId> {
    let url = Url::parse(uri.as_str())?;
    if url.scheme() == "file" {
        let path = crate::uri::to_site_path(uri.as_str())?;
        return path_id(&path, root).context("source is outside the site root");
    }
    package_url_id(&url)
}

pub(crate) fn package_file_id(uri: &Uri) -> Result<FileId> {
    package_url_id(&Url::parse(uri.as_str())?)
}

fn package_url_id(url: &Url) -> Result<FileId> {
    if url.scheme() != "tola-package"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("unsupported source URI");
    }
    let Some(id) = PACKAGE_FILE_ADDRESSES.get(url.as_str()) else {
        bail!("unknown embedded package source");
    };
    Ok(*id)
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

pub(crate) fn source_uri(id: FileId, root: &Path) -> Result<Uri> {
    let path = id.vpath().get_without_slash();
    if let VirtualRoot::Package(package) = id.root() {
        let mut uri = Url::parse("tola-package:/")?;
        uri.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid package URI base"))?
            .pop_if_empty()
            .push(&package.namespace)
            .push(&package.name)
            .push(&package.version.to_string())
            .extend(path.split('/'));
        crate::uri::from_url(uri)
    } else {
        crate::uri::from_file_path(&root.join(path))
    }
}

/// The address one file answers under, in the spelling the client named its root with.
///
/// A site file is addressed from the root the client named, never the resolved form the compiler
/// reads it through; a document of another root — a builtin package's own file — keeps its own
/// identity.
pub(crate) fn client_uri(id: FileId, client_root: &crate::uri::ClientRoot) -> Option<Uri> {
    if matches!(id.root(), VirtualRoot::Project) {
        return client_root
            .address(&client_root.resolved().join(id.vpath().get_without_slash()))
            .ok();
    }
    source_uri(id, client_root.resolved()).ok()
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
pub(crate) fn package_view_path(id: FileId, directory: &Path) -> Result<PathBuf> {
    let VirtualRoot::Package(spec) = id.root() else {
        bail!("source is not a package file");
    };
    Ok(directory
        .join(spec.namespace.as_str())
        .join(spec.name.as_str())
        .join(spec.version.to_string())
        .join(id.vpath().get_without_slash()))
}

/// Whether the file an editor reads has exactly the package source it mirrors.
///
/// A mirror some other Tola wrote holds positions the builtin source does not, so the bytes decide
/// whether the file is this implementation's package source at all.
pub(crate) fn verify_package_view(path: &Path, source: &str) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| {
        "Tola could not open an editor package source; run `tola editor setup`".to_owned()
    })?;
    let mut contents = Vec::with_capacity(source.len());
    file.take(source.len() as u64 + 1)
        .read_to_end(&mut contents)?;
    anyhow::ensure!(
        contents == source.as_bytes(),
        "an editor package source differs from this Tola implementation; run `tola editor setup`"
    );
    Ok(())
}

/// The builtin package source a file of the editor's package view mirrors, when it is one.
///
/// The mirror is not a source of its own: every answer about it is the immutable package source's,
/// taken from this implementation rather than from the copy the editor holds. A path no builtin
/// package file mirrors — an unknown package, a file that package does not hold, or a copy that
/// differs from the one this Tola ships — mirrors nothing.
pub(crate) fn package_view_id(path: &Path, directory: &Path) -> Option<FileId> {
    // The view is a spelling of the published files, not a place: an editor may reach the same
    // source through a link, and the site root may itself be spelled differently.
    let path = tola_build::filesystem::normalize_existing_prefix(path);
    let directory = tola_build::filesystem::normalize_existing_prefix(directory);
    // Every published file sits below the view directory, so a path outside it mirrors nothing:
    // this is the common answer, and it skips deriving every candidate address.
    if !path.starts_with(&directory) {
        return None;
    }
    for package in resolvable_packages() {
        for (file, source) in package.files() {
            let Some(id) = builtin_file_id(package, file) else {
                continue;
            };
            if package_view_path(id, &directory).ok().as_deref() != Some(path.as_path()) {
                continue;
            }
            return verify_package_view(&path, &source).is_ok().then_some(id);
        }
    }
    None
}

/// The builtin package source a file of the site's own package view mirrors, when it is one.
///
/// `tola editor setup` publishes a site's packages beside its configuration, and a file-only client
/// reads and is sent into those files. The site's own view needs no client configuration: a mirror
/// answers as the package document it mirrors, whether or not the client named the view.
pub(crate) fn mirrored_package_id(path: &Path, root: &Path) -> Option<FileId> {
    let view = root.join(tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY);
    package_view_id(path, &view)
}

/// Where one package document's bytes are read from, among the locations the compiler resolved.
///
/// A builtin source is embedded, every other package — `@preview`, a local package, a vendored one —
/// is a file below a package location. The order follows the compiler's: declared, data, then cache.
pub(crate) fn package_file_path(
    id: FileId,
    locations: &tola_typst::PackageLocations,
) -> Option<PathBuf> {
    let VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    let vpath = id.vpath().get_without_slash();
    locations
        .declared()
        .iter()
        .chain(locations.data())
        .chain(locations.cache())
        .map(|location| {
            location
                .root()
                .join(spec.namespace.as_str())
                .join(spec.name.as_str())
                .join(spec.version.to_string())
                .join(vpath)
        })
        .find(|path| path.is_file())
}

/// The package document a file below a package directory names, when the directory is one.
///
/// The rule lives in `tola-build`, beside the package locations it serves; the language server adds
/// no location requirement of its own, because the source boundary already governs reads.
pub(crate) fn package_directory_id(path: &Path) -> Option<FileId> {
    tola_build::filesystem::package_document_id(path, None)
}

/// Resolve with Typst's path semantics, then require an actual builtin file.
pub(crate) fn embedded_relative(id: FileId, path: &str) -> Option<FileId> {
    embedded_source(id)?;
    let target = tola_typst::typst::foundations::PathOrStr::Str(path.into())
        .resolve(id)
        .ok()?
        .intern();
    embedded_source(target).map(|_| target)
}

fn is_tola_package(id: FileId) -> bool {
    matches!(id.root(), VirtualRoot::Package(spec) if TolaPackage::from_spec(spec).is_some())
}

pub(crate) fn is_tola_span(span: tola_typst::typst::syntax::Span) -> bool {
    span.id().is_some_and(is_tola_package)
}
