//! Filesystem path identities, source observations, temporary directories, writes, and
//! whole-tree replacement.

use std::path::Path;

mod dev_server;
mod file_handle;
mod identity;
pub mod layout;
mod lock;
mod observation;
mod output_boundary;
mod overrides;
mod owned;
mod path;
mod read;
mod swap;
mod sys;
mod write;

/// Disposable local work; formal site inputs must not depend on this directory.
pub const INTERNAL_DIR: &str = ".tola";

/// The shape the address file [`layout::dev_server_address`] names is written in.
pub const DEV_SERVER_ADDRESS_VERSION: u32 = 1;

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

pub use dev_server::DevServerAddress;
pub(crate) use file_handle::open_regular_file;
pub use identity::FilesystemSourceIdentity;
pub(crate) use identity::{FilesystemSourceKind, FilesystemWatchEvidence};
pub(crate) use lock::{FileLock, FileLockError};
pub(crate) use observation::{
    FilesystemSourceFile, FilesystemSourceObservation, SelectedTreeObservation, TreeObservation,
    observe_file_source, observe_tree_source, observe_tree_source_selected, walk_tree_members,
    walk_tree_members_selected,
};
pub use output_boundary::OutputBoundaryError;
pub(crate) use output_boundary::{
    DeclaredOutputBoundaries, DeclaredOutputViolation, OutputBoundary,
};
pub(crate) use overrides::SourceOverrides;
pub use owned::owned_files;
pub use path::SitePathError;
pub(crate) use path::{absolute_path, path_failure_reason, root_relative};
pub(crate) use path::{canonical_site_root, render_relative, site_relative_absolute};
pub use path::{
    display_path, lexical_path_identity, normalize_existing_prefix, normalize_path, path_is_within,
};
pub(crate) use read::{FileReadError, read_chunks, read_file_chunks};
pub use swap::{SwapError, SwapMember, TreeSwap, swap_workspace};
pub(crate) use sys::{automatic_path_is_link_like, encode_path_identity};
pub use write::atomic_write;

mod fingerprint;
pub(crate) use fingerprint::{
    PathContents, PathFingerprint, digest_file, entries_fingerprint, path_contents,
    path_fingerprint,
};

mod directory;
pub(crate) use directory::{TemporaryDirectory, directory_is_empty, read_sorted_entries};

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
