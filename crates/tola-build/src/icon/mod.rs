//! Icon resources frozen before Bundle compilation.

pub(crate) mod iconify;
mod input;
pub(crate) mod output;
mod remote;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::{IconCollectionSource, IconsConfig};
use crate::filesystem::SourceOverrides;
use tola_diagnostic::{Diagnostic, Severity};

use input::{IconFileRead, SvgDirectoryRead};

/// An immutable set of icon collections and its local input evidence.
#[derive(Debug, Clone)]
pub(crate) struct IconInventory {
    collections: Arc<tola_icons::IconCollections>,
    resolved_collections: Arc<BTreeMap<String, Arc<ResolvedCollection>>>,
    evidence: IconInventoryEvidence,
}

#[derive(Debug)]
struct ResolvedCollection {
    collection: Arc<tola_icons::IconCollection>,
    reads: CollectionReads,
}

#[derive(Debug)]
enum CollectionReads {
    Pinned,
    Json(IconFileRead),
    SvgDirectory {
        membership: SvgDirectoryRead,
        files: BTreeMap<PathBuf, SvgSource>,
    },
}

#[derive(Debug, Clone)]
struct SvgSource {
    read: IconFileRead,
    icon: tola_icons::SvgIcon,
}

impl IconInventory {
    pub(crate) fn collections(&self) -> Arc<tola_icons::IconCollections> {
        Arc::clone(&self.collections)
    }

    pub(crate) fn evidence(&self) -> &IconInventoryEvidence {
        &self.evidence
    }
}

/// The local files and directory membership one icon preparation consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconInventoryEvidence {
    configuration: IconsConfig,
    internal_root: PathBuf,
    files: Arc<[IconFileRead]>,
    directories: Arc<[SvgDirectoryRead]>,
    boundary: tola_typst::SourceBoundary,
    vendor_root: Option<PathBuf>,
}

impl IconInventoryEvidence {
    pub(crate) fn watch_evidence(&self) -> crate::filesystem::FilesystemWatchEvidence {
        use crate::filesystem::{FilesystemSourceKind, FilesystemWatchEvidence};
        FilesystemWatchEvidence::from_sources(
            self.files
                .iter()
                .map(|read| (FilesystemSourceKind::File, read.source.clone()))
                .chain(
                    self.directories
                        .iter()
                        .map(|read| (FilesystemSourceKind::Tree, read.source.clone())),
                ),
        )
    }

    pub(crate) fn is_fresh(&self, cancellation: &BuildCancellation) -> anyhow::Result<bool> {
        cancellation.ensure_active()?;
        let overrides = SourceOverrides::default();
        for read in self.files.iter() {
            if !read.is_fresh(&overrides, cancellation)? {
                return Ok(false);
            }
        }
        for read in self.directories.iter() {
            if !read.is_fresh(&overrides, cancellation)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn is_fresh_for(
        &self,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<bool> {
        cancellation.ensure_active()?;
        if self.vendor_root != config.vendor.path {
            return Ok(false);
        }
        if self.configuration != config.icons
            || self.internal_root
                != crate::filesystem::normalize_existing_prefix(
                    &config.get_root().join(crate::filesystem::INTERNAL_DIR),
                )
        {
            return Ok(false);
        }
        self.is_fresh(cancellation)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IconError {
    #[error("Tola could not read an icon source file")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("an icon source file changed while it was being read")]
    Changed { path: PathBuf },
    #[error("an icon source file exceeds the {limit}-byte collection limit")]
    TooLarge { path: PathBuf, limit: usize },
    #[error("an SVG file in the icon source has no usable icon name")]
    InvalidName { path: PathBuf },
    #[error("the icon source contains a link instead of a file")]
    LinkedDirectoryMember { path: PathBuf },
    #[error("an icon source is inside the reserved `.tola` directory")]
    InternalSource { path: PathBuf },
    #[error("icon collection `{namespace}` is not valid")]
    Collection {
        namespace: String,
        path: Option<PathBuf>,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("Tola could not download icon collection `{namespace}`")]
    Remote {
        namespace: String,
        /// Every address the collection was fetched from, in the order tried.
        urls: Vec<String>,
        #[source]
        source: crate::HttpDownloadError,
    },
    #[error("icon collection `{namespace}` from `{url}` exceeds the {limit}-byte collection limit")]
    RemoteTooLarge {
        namespace: String,
        url: String,
        limit: usize,
    },
    #[error(
        "network access is disabled, so Tola cannot download icon collection `{namespace}` from `{url}`"
    )]
    NetworkDenied { namespace: String, url: String },
    #[error(
        "SHA-256 mismatch for icon collection `{namespace}` from `{url}`: expected {expected}, received {received}"
    )]
    Integrity {
        namespace: String,
        url: String,
        expected: String,
        received: String,
        size_expected: u64,
        size_received: u64,
        path: Option<PathBuf>,
    },
    /// A configured icon source lies outside what this build may read.
    #[error("Tola may not read icon source `{path}`")]
    Boundary {
        path: PathBuf,
        #[source]
        source: typst::diag::FileError,
    },
    /// The npm registry returned metadata this build cannot use.
    #[error("the npm registry returned no usable metadata for preset `{preset}`")]
    RegistryFormat { preset: String },
    /// A preset names a release the registry does not publish.
    #[error("preset `{preset}` has no release `{version}`")]
    RegistryRelease {
        preset: String,
        version: String,
        latest: Option<String>,
    },
    /// A published release carries no usable integrity digest.
    #[error("preset `{preset}` release `{version}` publishes no integrity digest")]
    RegistryIntegrity { preset: String, version: String },
    /// The registry metadata request failed.
    #[error("Tola could not read the npm registry metadata for preset `{preset}`")]
    RegistryMetadata {
        namespace: String,
        preset: String,
        url: String,
        #[source]
        source: crate::HttpDownloadError,
    },
    /// A downloaded tarball does not match the digest the registry published.
    #[error(
        "the tarball of preset `{preset}` release `{version}` does not match its published integrity: expected {expected}, received {received}"
    )]
    TarballIntegrity {
        namespace: String,
        preset: String,
        version: String,
        expected: String,
        received: String,
    },
    /// A downloaded tarball could not be read.
    #[error("Tola could not read the tarball of preset `{preset}` release `{version}`")]
    Tarball {
        namespace: String,
        preset: String,
        version: String,
    },
    /// A tarball's collection file exceeds the collection limit.
    #[error(
        "the tarball of preset `{preset}` release `{version}` exceeds the {limit}-byte collection limit"
    )]
    TarballCollectionTooLarge {
        namespace: String,
        preset: String,
        version: String,
        limit: usize,
    },
    /// A tarball holds no collection file.
    #[error("the tarball of preset `{preset}` release `{version}` holds no collection file")]
    TarballCollection {
        namespace: String,
        preset: String,
        version: String,
    },
    /// A locked collection is absent and this build may not fetch it.
    #[error("icon collection `{namespace}` is not locked; run `tola update {namespace}`")]
    Unlocked { namespace: String },
    /// A declaration no longer matches the lock entry.
    #[error(
        "icon collection `{namespace}` no longer matches `tola.lock`; run `tola update {namespace}`"
    )]
    LockMismatch {
        namespace: String,
        recorded: String,
        declared: String,
    },
    /// A locked collection has no bytes this build may read.
    #[error("icon collection `{namespace}` has no bytes for digest {sha256}; run `tola vendor`")]
    MissingBytes {
        namespace: String,
        sha256: crate::inputs::Sha256Digest,
    },
    /// The declaration names neither a preset nor a URL.
    #[error("icon collection `{namespace}` names neither a preset nor a URL")]
    NotRemote { namespace: String },
    #[error("build cancelled")]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error("Tola could not download the configured icon collections")]
    NetworkRuntime(#[source] std::io::Error),
    #[error("Tola could not finish downloading an icon collection")]
    NetworkWorker,
}

impl IconError {
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Read { path, .. }
            | Self::Changed { path }
            | Self::TooLarge { path, .. }
            | Self::InvalidName { path }
            | Self::LinkedDirectoryMember { path }
            | Self::InternalSource { path } => Some(path),
            Self::Collection { path, .. } => path.as_deref(),
            Self::Integrity { path, .. } => path.as_deref(),
            Self::Remote { .. }
            | Self::RemoteTooLarge { .. }
            | Self::NetworkDenied { .. }
            | Self::NetworkRuntime(_)
            | Self::NetworkWorker
            | Self::Cancelled(_) => None,
            _ => None,
        }
    }
}

/// One invalid icon collection, attributed to `path` when a single file owns it.
fn collection_error(
    namespace: &str,
    path: Option<&Path>,
    source: impl std::error::Error + Send + Sync + 'static,
) -> IconError {
    IconError::Collection {
        namespace: namespace.to_owned(),
        path: path.map(Path::to_path_buf),
        source: Box::new(source),
    }
}

/// The distinct hosts a failed download tried, for one readable line.
fn tried_hosts(urls: &[String]) -> String {
    let mut hosts: Vec<String> = Vec::new();
    for url in urls {
        let host = url::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_else(|| url.clone());
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
        .iter()
        .map(|host| format!("`{host}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first line of a collection parser's own reason for rejecting its input.
fn collection_failure_reason(source: &(dyn std::error::Error + Send + Sync)) -> Option<String> {
    source
        .to_string()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

pub(crate) fn error_diagnostic(error: &anyhow::Error, root: &Path) -> Option<Diagnostic> {
    let error = error
        .chain()
        .find_map(|source| source.downcast_ref::<IconError>())?;
    let diagnostic = Diagnostic::new(
        tola_diagnostic::codes::icons::COLLECTION,
        Severity::Error,
        error.to_string(),
    );
    let diagnostic = match error {
        IconError::Read { source, .. } => diagnostic
            .with_note(crate::filesystem::path_failure_reason(source))
            .with_help("Check that the icon source exists and that Tola can read it"),
        IconError::Changed { .. } => diagnostic
            .with_help("Run the build again without changing icon sources"),
        IconError::TooLarge { .. } => {
            diagnostic.with_help("Split the icons across several `icons.collections` entries")
        }
        IconError::InvalidName { .. } => diagnostic.with_note(
            "icon names drop the `.svg` extension and write `/` as `-`",
        ),
        IconError::LinkedDirectoryMember { .. } => diagnostic.with_help(
            "Replace the link with the SVG file it points at, or declare that file as its own `icons.collections` entry",
        ),
        IconError::InternalSource { .. } => {
            diagnostic.with_help("Move the icon source outside `.tola` and update `icons.collections`")
        }
        IconError::Collection { source, .. } => match collection_failure_reason(source.as_ref()) {
            Some(reason) => diagnostic.with_note(reason),
            None => diagnostic,
        },
        IconError::Remote { urls, .. } => {
            let diagnostic = match urls.len() {
                2.. => diagnostic.with_note(format!("Tola tried {}", tried_hosts(urls))),
                _ => diagnostic,
            };
            diagnostic.with_help("Check the connection and the address")
        }
        IconError::RemoteTooLarge { .. } => diagnostic.with_help(
            "Use a smaller collection, or declare the icons as a local file in `icons.collections`",
        ),
        IconError::NetworkDenied { .. } => diagnostic.with_help(
            "Run `tola vendor` while online, or declare a local icon collection",
        ),
        IconError::Integrity { .. } => diagnostic
            .with_note("update `sha256` only when you intend to publish the bytes it now serves")
            .with_help("Run `tola vendor --refresh` to restore the site's copy"),
        IconError::NetworkRuntime(_) => {
            diagnostic.with_help("Check the connection, then run the build again")
        }
        IconError::NetworkWorker => diagnostic.with_help("Run the build again"),
        _ => diagnostic,
    };
    Some(match error.path() {
        Some(path) => diagnostic.with_path(crate::filesystem::display_path(path, root)),
        None => diagnostic,
    })
}

/// Observe each configured collection, reusing imports whose evidence still holds.
pub(crate) fn prepare(
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
    previous: Option<&IconInventory>,
    resources: &crate::resources::BuildResources,
    overrides: &SourceOverrides,
) -> anyhow::Result<IconInventory> {
    cancellation.ensure_active()?;
    let internal = crate::filesystem::normalize_existing_prefix(
        &config.get_root().join(crate::filesystem::INTERNAL_DIR),
    );
    let boundary = resources.source_boundary(config);
    let previous = previous.filter(|inventory| {
        inventory.evidence.internal_root == internal
            && inventory.evidence.boundary == boundary
            && inventory.evidence.vendor_root == config.vendor.path
    });
    let mut unchanged =
        previous.is_some_and(|inventory| inventory.evidence.configuration == config.icons);
    let mut resolved_collections = BTreeMap::new();
    for (namespace, source) in &config.icons.collections {
        cancellation.ensure_active()?;
        let retained = previous
            .filter(|inventory| {
                inventory.evidence.configuration.collections.get(namespace) == Some(source)
            })
            .and_then(|inventory| inventory.resolved_collections.get(namespace));
        let collection = prepare_collection(
            namespace,
            &CollectionInputs {
                config,
                internal: &internal,
                cancellation,
                resources,
                overrides,
                boundary: &boundary,
            },
            retained,
        )?;
        unchanged &= retained.is_some_and(|retained| Arc::ptr_eq(retained, &collection));
        resolved_collections.insert(namespace.clone(), collection);
    }
    cancellation.ensure_active()?;
    if unchanged {
        return Ok(previous
            .expect("unchanged collections have an accepted inventory")
            .clone());
    }

    let mut collections = tola_icons::IconCollections::new();
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for (namespace, inventory) in &resolved_collections {
        cancellation.ensure_active()?;
        match &inventory.reads {
            CollectionReads::Pinned => {}
            CollectionReads::Json(read) => files.push(read.clone()),
            CollectionReads::SvgDirectory {
                membership,
                files: members,
            } => {
                directories.push(membership.clone());
                files.extend(members.values().map(|source| source.read.clone()));
            }
        }
        collections
            .mount(namespace, Arc::clone(&inventory.collection))
            .map_err(|source| collection_error(namespace, None, source))?;
    }
    files.sort_by(|left, right| left.source.logical_path().cmp(right.source.logical_path()));
    directories.sort_by(|left, right| left.source.logical_path().cmp(right.source.logical_path()));
    let evidence = IconInventoryEvidence {
        configuration: config.icons.clone(),
        internal_root: internal,
        files: files.into(),
        directories: directories.into(),
        boundary,
        vendor_root: config.vendor.path.clone(),
    };
    let collections = Arc::new(collections);
    cancellation.ensure_active()?;
    Ok(IconInventory {
        collections,
        resolved_collections: Arc::new(resolved_collections),
        evidence,
    })
}

/// The request one configured remote collection sends.
fn remote_request<'a>(
    config: &'a ResolvedSiteConfig,
    namespace: &'a str,
    cancellation: &'a BuildCancellation,
    resources: &'a crate::resources::BuildResources,
    boundary: &'a tola_typst::SourceBoundary,
) -> anyhow::Result<remote::RemoteCollection<'a>> {
    let Some(IconCollectionSource::RemoteJson {
        preset,
        version,
        url,
        mirrors,
    }) = config.icons.collections.get(namespace)
    else {
        anyhow::bail!("`{namespace}` is not a configured remote icon collection");
    };
    let lock = crate::inputs::InputLock::read(config.get_root())?.unwrap_or_default();
    Ok(remote::RemoteCollection {
        namespace,
        preset: preset.as_deref(),
        version: version.as_deref(),
        url: url.as_deref(),
        mirrors: mirrors.as_deref().unwrap_or_default(),
        locked: lock.icons.get(namespace).cloned(),
        vendored: config.vendor.icon_collection(namespace),
        cache_root: crate::BuildResources::icon_cache_directory(config.get_root()),
        mode: resources.input_mode(),
        cancellation,
        resources,
        boundary,
    })
}

/// The verified bytes of one configured remote icon collection.
pub fn remote_collection_bytes(
    config: &ResolvedSiteConfig,
    namespace: &str,
    resources: &crate::resources::BuildResources,
    cancellation: &BuildCancellation,
) -> anyhow::Result<Vec<u8>> {
    let boundary = resources.source_boundary(config);
    let request = remote_request(config, namespace, cancellation, resources, &boundary)?;
    let (_, verified) = remote::load_collection(request)?;
    Ok(verified.bytes)
}

/// The site-wide facts every collection in one build shares.
struct CollectionInputs<'a> {
    config: &'a ResolvedSiteConfig,
    internal: &'a Path,
    cancellation: &'a BuildCancellation,
    resources: &'a crate::resources::BuildResources,
    overrides: &'a SourceOverrides,
    boundary: &'a tola_typst::SourceBoundary,
}

fn prepare_collection(
    namespace: &str,
    context: &CollectionInputs<'_>,
    previous: Option<&Arc<ResolvedCollection>>,
) -> anyhow::Result<Arc<ResolvedCollection>> {
    let CollectionInputs {
        config,
        internal,
        cancellation,
        resources,
        overrides,
        boundary,
    } = *context;
    let (collection, reads) = match &config.icons.collections[namespace] {
        IconCollectionSource::RemoteJson { .. } => {
            let request = remote_request(config, namespace, cancellation, resources, boundary)?;
            if let Some(previous) = previous {
                let reusable = match &previous.reads {
                    CollectionReads::Pinned => resources.input_scope() != crate::InputScope::Pure,
                    CollectionReads::Json(read) => read.is_fresh(overrides, cancellation)?,
                    CollectionReads::SvgDirectory { .. } => false,
                };
                if reusable {
                    return Ok(Arc::clone(previous));
                }
            }
            let (collection, verified) = remote::load_collection(request)?;
            (
                Arc::new(collection),
                verified
                    .source
                    .map_or(CollectionReads::Pinned, CollectionReads::Json),
            )
        }
        IconCollectionSource::LocalJson { path } => {
            reject_internal_source(path, internal)?;
            boundary.check(path)?;
            if let Some(previous) = previous
                && let CollectionReads::Json(read) = &previous.reads
                && read.is_fresh(overrides, cancellation)?
            {
                return Ok(Arc::clone(previous));
            }
            let (bytes, read) = input::read_file(path, overrides, cancellation, boundary)?;
            (
                Arc::new(parse_collection(namespace, Some(path), &bytes)?),
                CollectionReads::Json(read),
            )
        }
        IconCollectionSource::LocalSvgDir { path } => {
            return prepare_directory(namespace, path, context, previous);
        }
    };
    cancellation.ensure_active()?;
    Ok(Arc::new(ResolvedCollection { collection, reads }))
}

fn prepare_directory(
    namespace: &str,
    directory: &Path,
    context: &CollectionInputs<'_>,
    previous: Option<&Arc<ResolvedCollection>>,
) -> anyhow::Result<Arc<ResolvedCollection>> {
    let CollectionInputs {
        internal,
        cancellation,
        overrides,
        boundary,
        ..
    } = *context;
    reject_internal_source(directory, internal)?;
    let (paths, membership) = input::svg_paths(directory, overrides, cancellation, boundary)?;
    let retained = previous.and_then(|inventory| match &inventory.reads {
        CollectionReads::SvgDirectory { membership, files } => Some((membership, files)),
        _ => None,
    });
    let mut unchanged = retained.is_some_and(|(previous, _)| previous == &membership);
    let mut files = BTreeMap::new();
    let mut collection_bytes = 0_usize;
    for path in paths {
        cancellation.ensure_active()?;
        reject_internal_source(&path, internal)?;
        boundary.check(&path)?;
        let retained = retained.and_then(|(_, files)| files.get(&path));
        let source = if let Some(retained) = retained
            && retained.read.is_fresh(overrides, cancellation)?
        {
            Cow::Borrowed(retained)
        } else {
            unchanged = false;
            let name = input::icon_name(directory, &path)?;
            let (bytes, read) = input::read_file(&path, overrides, cancellation, boundary)?;
            let icon = tola_icons::SvgIcon::parse(&bytes).map_err(|source| {
                collection_error(
                    namespace,
                    Some(&path),
                    tola_icons::InvalidCollection::Svg { name, source },
                )
            })?;
            Cow::Owned(SvgSource { read, icon })
        };
        collection_bytes = collection_bytes.saturating_add(source.read.byte_len);
        input::ensure_collection_size(directory, collection_bytes as u64)?;
        files.insert(path, source);
    }
    cancellation.ensure_active()?;
    if unchanged {
        return Ok(Arc::clone(
            previous.expect("unchanged directory has an accepted collection"),
        ));
    }
    let files = files
        .into_iter()
        .map(|(path, source)| (path, source.into_owned()))
        .collect::<BTreeMap<_, _>>();
    let mut collection = tola_icons::IconCollection::new();
    for (path, source) in &files {
        cancellation.ensure_active()?;
        let name = input::icon_name(directory, path)?;
        collection
            .insert(&name, source.icon.clone())
            .map_err(|source| collection_error(namespace, Some(path), source))?;
    }
    Ok(Arc::new(ResolvedCollection {
        collection: Arc::new(collection),
        reads: CollectionReads::SvgDirectory { membership, files },
    }))
}

fn parse_collection(
    namespace: &str,
    path: Option<&Path>,
    bytes: &[u8],
) -> Result<tola_icons::IconCollection, IconError> {
    tola_icons::IconCollection::from_iconify(bytes)
        .map_err(|source| collection_error(namespace, path, source))
}

fn reject_internal_source(path: &Path, internal: &Path) -> Result<(), IconError> {
    if crate::filesystem::normalize_existing_prefix(path).starts_with(internal) {
        return Err(IconError::InternalSource {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Configured inputs also remain watchable after a missing or invalid resource aborts a build.
pub(crate) fn configured_collection_watch_evidence(
    config: &ResolvedSiteConfig,
) -> crate::filesystem::FilesystemWatchEvidence {
    use crate::filesystem::{
        FilesystemSourceIdentity, FilesystemSourceKind, FilesystemWatchEvidence,
    };
    FilesystemWatchEvidence::from_sources(config.icons.collections.values().filter_map(|source| {
        let (kind, path) = match source {
            IconCollectionSource::LocalJson { path } => (FilesystemSourceKind::File, path),
            IconCollectionSource::LocalSvgDir { path } => (FilesystemSourceKind::Tree, path),
            IconCollectionSource::RemoteJson { .. } => return None,
        };
        Some((kind, FilesystemSourceIdentity::from_path(path)))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brand_icons_config(root: &Path) -> ResolvedSiteConfig {
        crate::config::tests::load_test_config(root, BRAND_COLLECTION_TOML)
    }

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M0 0h24v24H0z"/></svg>"#;

    const BRAND_COLLECTION_TOML: &str = r#"
            [icons.collections.brand]
            source-type = "local-svg-dir"
            path = "brand"
        "#;

    fn prepare_icons(
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
        previous: Option<&IconInventory>,
    ) -> anyhow::Result<IconInventory> {
        prepare(
            config,
            cancellation,
            previous,
            &crate::resources::BuildResources::default(),
            &SourceOverrides::default(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn missing_svg_directory_stays_watched() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(
            directory.path(),
            r#"
            [icons.collections.brand]
            source-type = "local-svg-dir"
            path = "missing/brand"
        "#,
        );
        assert!(prepare_icons(&config, &BuildCancellation::new(), None,).is_err());
        let evidence = configured_collection_watch_evidence(&config);
        let [boundary] = evidence.boundaries() else {
            panic!("expected one source boundary")
        };
        assert_eq!(
            boundary.kind(),
            crate::filesystem::FilesystemSourceKind::Tree
        );
        assert!(boundary.logical_path().ends_with("missing/brand"));
    }

    #[cfg(unix)]
    #[test]
    fn reuse_rejects_internal_sources() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("brand")).unwrap();
        std::fs::write(root.join("brand/mark.svg"), SVG).unwrap();
        let config = brand_icons_config(root);
        let inventory = prepare_icons(&config, &BuildCancellation::new(), None).unwrap();
        symlink(
            root.join("brand"),
            root.join(crate::filesystem::INTERNAL_DIR),
        )
        .unwrap();

        let error =
            prepare_icons(&config, &BuildCancellation::new(), Some(&inventory)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::InternalSource { .. })
        ));
        assert!(inventory.collections().get("brand", "mark").is_some());
    }
}
