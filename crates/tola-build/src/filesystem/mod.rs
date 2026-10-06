//! Filesystem path identities, source observations, temporary directories, and writes.

use std::path::Path;

mod file_handle;
mod identity;
mod lock;
mod output_boundary;
mod overrides;
mod path;
mod read;
mod snapshot;
mod sys;
mod write;

/// Disposable local work; formal site inputs must not depend on this directory.
pub const INTERNAL_DIR: &str = ".tola";

/// The package mirrors `tola editor setup` publishes for a site, below [`INTERNAL_DIR`].
///
/// A file-only client is sent into these files by a definition into a package, so the language
/// server reads the mirrors as sources while every other path below [`INTERNAL_DIR`] stays
/// generated state.
pub const PACKAGE_MIRROR_DIRECTORY: &str = ".tola/builtin-packages";

/// The address the running development server serves a site at, below [`INTERNAL_DIR`].
///
/// `tola dev` writes this file when it has bound its listener and removes it when it stops, so a
/// process beside it — the language server, which an editor starts separately — names the address
/// of one page without the author configuring it twice. A missing file, or one this version does
/// not read, names no development server.
pub const DEV_SERVER_STATE_FILE: &str = ".tola/dev-server.json";

/// The shape [`DEV_SERVER_STATE_FILE`] is written in.
pub const DEV_SERVER_STATE_VERSION: u32 = 1;

/// The package document a file below a package directory names, when the directory is one.
///
/// A definition into a package names the package's source wherever that package lives: the host
/// cache, a `--package-path` directory, or a vendored tree. The manifest decides — only a directory
/// that holds `typst.toml` is a package — so a path that merely looks like
/// `packages/<namespace>/<name>/<version>/…` names nothing here.
///
/// `locations` adds the caller's requirement that the package sits below a package root this build
/// resolved; the language server passes none, because the source boundary already governs which
/// paths a read may reach.
pub fn package_document_id(
    path: &Path,
    locations: Option<&tola_typst::PackageLocations>,
) -> Option<tola_typst::typst::syntax::FileId> {
    use tola_typst::typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

    let path = normalize_existing_prefix(path);
    let mut directory = path.parent()?;
    loop {
        if directory.join("typst.toml").is_file() {
            if let Some(locations) = locations
                && !locations
                    .declared()
                    .iter()
                    .chain(locations.data())
                    .chain(locations.cache())
                    .any(|location| directory.starts_with(location.root()))
            {
                return None;
            }
            let version = directory.file_name()?.to_str()?.parse().ok()?;
            let name = directory.parent()?.file_name()?.to_str()?;
            let namespace = directory.parent()?.parent()?.file_name()?.to_str()?;
            let vpath = VirtualPath::new(path.strip_prefix(directory).ok()?.to_str()?).ok()?;
            return Some(
                RootedPath::new(
                    VirtualRoot::Package(tola_typst::typst::syntax::package::PackageSpec {
                        namespace: namespace.into(),
                        name: name.into(),
                        version,
                    }),
                    vpath,
                )
                .intern(),
            );
        }
        directory = directory.parent()?;
    }
}

/// The pathname must remain in place while any process can hold the site lock.
pub const SITE_BUILD_LOCK_FILE: &str = ".tola-build.lock";

/// Publication recovery stays beside its output, independent of disposable local work.
pub fn publication_workspace(output: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut name = std::ffi::OsString::from(".");
    name.push(output.file_name()?);
    name.push("-publish");
    Some(output.with_file_name(name))
}

pub(crate) use file_handle::open_regular_file;
pub use identity::FilesystemSourceIdentity;
pub(crate) use identity::{FilesystemSourceKind, FilesystemWatchEvidence};
pub(crate) use lock::{FileLock, FileLockError};
pub(crate) use output_boundary::{
    DeclaredOutputBoundaries, DeclaredOutputViolation, OutputBoundary, OutputBoundaryError,
};
pub(crate) use overrides::SourceOverrides;
pub(crate) use path::{
    SitePathError, canonical_site_root, render_relative, site_relative_absolute,
};
pub(crate) use path::{absolute_path, path_failure_reason, root_relative};
pub use path::{
    display_path, lexical_path_identity, normalize_existing_prefix, normalize_path, path_is_within,
};
pub(crate) use read::{FileReadError, read_chunks, read_file_chunks};
pub(crate) use snapshot::{
    FilesystemSourceFile, FilesystemSourceObservation, observe_file_source, observe_tree_source,
    walk_tree_members,
};
pub(crate) use sys::{automatic_path_is_link_like, encode_path_identity};
pub use write::atomic_write;

mod fingerprint;
pub(crate) use fingerprint::{
    PathFingerprint, PathSnapshot, digest_file, entry_snapshot, path_fingerprint,
    snapshot_fingerprint,
};

mod directory;
pub(crate) use directory::{TemporaryDirectory, read_sorted_entries};

#[cfg(test)]
mod tests {
    use super::package_document_id;

    /// A directory that merely looks like a package names nothing without its manifest.
    #[test]
    fn package_directory_needs_its_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let version = directory.path().join("packages/preview/demo/1.0.0");
        std::fs::create_dir_all(&version).unwrap();
        let file = version.join("lib.typ");
        std::fs::write(&file, "").unwrap();
        assert!(package_document_id(&file, None).is_none());
        std::fs::write(version.join("typst.toml"), "").unwrap();
        assert!(package_document_id(&file, None).is_some());
    }
}
