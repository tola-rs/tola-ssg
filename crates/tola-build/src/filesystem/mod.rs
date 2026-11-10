//! Filesystem source observations, temporary directories, writes, and whole-tree replacement.

use std::path::Path;

use tola_input::normalize_existing_prefix;

mod dev_server;
mod file_handle;
mod identity;
mod lock;
mod observation;
mod output_boundary;
mod overrides;
mod owned;
mod path;
mod swap;
mod write;

/// The shape the address file [`tola_input::layout::dev_server_address`] names is written in.
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

/// Turn one shared filesystem read error into the build's error type.
pub(crate) trait FileReadErrorExt {
    fn into_anyhow(self) -> anyhow::Error;
}

impl FileReadErrorExt for tola_input::FileReadError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Io(source) => anyhow::Error::new(source),
            Self::Cancelled(_) => anyhow::Error::new(crate::cancellation::BuildCancelled),
        }
    }
}

pub use dev_server::DevServerAddress;
pub(crate) use identity::{FilesystemSourceKind, FilesystemWatchEvidence};
pub(crate) use lock::{FileLock, FileLockError};
pub(crate) use observation::{
    FilesystemSourceObservation, TreeObservation, observe_file_source,
    observe_tree_source_selected, walk_tree_members_selected,
};
pub use output_boundary::OutputBoundaryError;
pub(crate) use output_boundary::{
    DeclaredOutputBoundaries, DeclaredOutputViolation, OutputBoundary,
};
pub(crate) use overrides::SourceOverrides;
pub use owned::owned_files;
pub use path::{SitePathError, path_is_within};
pub(crate) use path::{absolute_path, path_failure_reason, root_relative};
pub(crate) use path::{canonical_site_root, site_relative_absolute};
pub use swap::{SwapError, SwapMember, TreeSwap, swap_workspace};
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
