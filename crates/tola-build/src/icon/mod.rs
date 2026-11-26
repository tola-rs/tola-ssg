//! Icon resources frozen before Bundle compilation.

mod error;
pub(crate) mod iconify;
mod input;
pub(crate) mod output;
mod remote;

pub(crate) use error::error_diagnostic;
pub use error::{IconError, IconIntegrityError};

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::BuildResources;
use crate::cancellation::BuildCancellation;
use crate::config::SiteConfig;
use crate::config::section::{IconCollectionSource, IconsConfig};
use crate::filesystem::SourceOverrides;
use tola_input::{InputLock, LockedIcon};

use error::collection_error;
use input::{IconFileRead, SvgDirectoryRead};

/// An immutable set of icon collections and its local input evidence.
#[derive(Debug, Clone)]
pub(crate) struct IconInventory {
    collections: Arc<tola_icons::IconCollections>,
    resolved_collections: Arc<BTreeMap<String, Arc<LoadedCollection>>>,
    evidence: IconSnapshot,
}

#[derive(Debug)]
struct LoadedCollection {
    collection: Arc<tola_icons::IconCollection>,
    reads: CollectionReads,
}

#[derive(Debug)]
enum CollectionReads {
    Remote {
        locked: LockedIcon,
        source: Option<IconFileRead>,
    },
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

    pub(crate) fn evidence(&self) -> &IconSnapshot {
        &self.evidence
    }
}

/// The local files and directory membership one icon preparation consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconSnapshot {
    configuration: IconsConfig,
    internal_root: PathBuf,
    files: Arc<[IconFileRead]>,
    directories: Arc<[SvgDirectoryRead]>,
    boundary: tola_typst::SourceBoundary,
    vendor_root: Option<PathBuf>,
    remote: BTreeMap<String, LockedIcon>,
}

impl IconSnapshot {
    pub(crate) fn watch_evidence(&self) -> crate::filesystem::WatchPaths {
        use crate::filesystem::{FilesystemSourceKind, WatchPaths};
        WatchPaths::from_sources(
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
        config: &SiteConfig,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<bool> {
        cancellation.ensure_active()?;
        if !self.remote.is_empty() {
            let lock = InputLock::read(config.root())?.unwrap_or_default();
            if self
                .remote
                .iter()
                .any(|(name, entry)| lock.icons.get(name) != Some(entry))
            {
                return Ok(false);
            }
        }
        if self.vendor_root != config.vendor.path {
            return Ok(false);
        }
        if self.configuration != config.icons
            || self.internal_root
                != tola_input::normalize_existing_prefix(
                    &config.root().join(tola_input::INTERNAL_DIR),
                )
        {
            return Ok(false);
        }
        self.is_fresh(cancellation)
    }
}

/// Observe each configured collection, reusing imports whose evidence still holds.
pub(crate) fn prepare(
    config: &SiteConfig,
    cancellation: &BuildCancellation,
    previous: Option<&IconInventory>,
    resources: &crate::resources::BuildResources,
    overrides: &SourceOverrides,
) -> anyhow::Result<IconInventory> {
    cancellation.ensure_active()?;
    let internal =
        tola_input::normalize_existing_prefix(&config.root().join(tola_input::INTERNAL_DIR));
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
    let mut remote = BTreeMap::new();
    for (namespace, inventory) in &resolved_collections {
        cancellation.ensure_active()?;
        match &inventory.reads {
            CollectionReads::Remote { locked, source } => {
                remote.insert(namespace.clone(), locked.clone());
                files.extend(source.iter().cloned());
            }
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
    let evidence = IconSnapshot {
        configuration: config.icons.clone(),
        internal_root: internal,
        files: files.into(),
        directories: directories.into(),
        boundary,
        vendor_root: config.vendor.path.clone(),
        remote,
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
    config: &'a SiteConfig,
    namespace: &'a str,
    cancellation: &'a BuildCancellation,
    resources: &'a crate::resources::BuildResources,
    boundary: &'a tola_typst::SourceBoundary,
    locked: Option<LockedIcon>,
) -> Result<remote::RemoteCollection<'a>, IconError> {
    let Some(IconCollectionSource::RemoteJson {
        preset,
        version,
        url,
        mirrors,
    }) = config.icons.collections.get(namespace)
    else {
        return Err(IconError::NotRemote {
            namespace: namespace.into(),
        });
    };
    Ok(remote::RemoteCollection {
        namespace,
        preset: preset.as_deref(),
        version: version.as_deref(),
        url: url.as_deref(),
        mirrors: mirrors.as_deref().unwrap_or_default(),
        locked,
        vendored: config.vendor.icon_collection(namespace),
        cache_root: crate::BuildResources::icon_cache_directory(config.root()),
        mode: resources.input_mode(),
        cancellation,
        resources,
        boundary,
    })
}

/// The verified bytes of one configured remote icon collection.
pub fn remote_collection_bytes(
    config: &SiteConfig,
    namespace: &str,
    resources: &crate::resources::BuildResources,
    cancellation: &BuildCancellation,
) -> Result<Vec<u8>, IconError> {
    let boundary = resources.source_boundary(config);
    let original = InputLock::read(config.root())?.unwrap_or_default();
    let request = remote_request(
        config,
        namespace,
        cancellation,
        resources,
        &boundary,
        original.icons.get(namespace).cloned(),
    )?;
    let (_, verified) = load_remote(config, &original, request)?;
    Ok(verified.bytes)
}

fn load_remote(
    config: &SiteConfig,
    original: &InputLock,
    request: remote::RemoteCollection<'_>,
) -> Result<(tola_icons::IconCollection, remote::CollectionBytes), IconError> {
    let namespace = request.namespace;
    let mode = request.mode;
    let cancellation = request.cancellation;
    let (collection, verified) = remote::load_collection(request)?;
    cancellation.ensure_active()?;
    if mode.may_write_lock() && original.icons.get(namespace) != Some(&verified.locked) {
        let mut updated = original.clone();
        updated
            .icons
            .insert(namespace.into(), verified.locked.clone());
        updated.commit(config.root(), original)?;
    }
    Ok((collection, verified))
}

/// Fetch and validate selected remote collections, recording entries without writing the lock.
/// Calls `started` before reading each collection.
pub fn refresh_icon_collections(
    config: &SiteConfig,
    names: &[String],
    resources: &BuildResources,
    cancellation: &BuildCancellation,
    lock: &mut InputLock,
    mut started: impl FnMut(&str),
) -> Result<(), IconError> {
    if !resources.input_mode().may_write_lock() {
        return Err(IconError::RefreshLocked);
    }
    let boundary = resources.source_boundary(config);
    for name in names {
        cancellation.ensure_active()?;
        started(name);
        let request = remote_request(config, name, cancellation, resources, &boundary, None)?;
        let (_, verified) = remote::load_collection(request)?;
        cancellation.ensure_active()?;
        lock.icons.insert(name.clone(), verified.locked);
    }
    Ok(())
}

/// The site-wide facts every collection in one build shares.
struct CollectionInputs<'a> {
    config: &'a SiteConfig,
    internal: &'a Path,
    cancellation: &'a BuildCancellation,
    resources: &'a crate::resources::BuildResources,
    overrides: &'a SourceOverrides,
    boundary: &'a tola_typst::SourceBoundary,
}

fn prepare_collection(
    namespace: &str,
    context: &CollectionInputs<'_>,
    previous: Option<&Arc<LoadedCollection>>,
) -> anyhow::Result<Arc<LoadedCollection>> {
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
            let original = InputLock::read(config.root())?.unwrap_or_default();
            let request = remote_request(
                config,
                namespace,
                cancellation,
                resources,
                boundary,
                original.icons.get(namespace).cloned(),
            )?;
            if let Some(previous) = previous {
                let reusable = match &previous.reads {
                    CollectionReads::Remote { locked, source }
                        if request.locked.as_ref() == Some(locked) =>
                    {
                        match source {
                            Some(read) => read.is_fresh(overrides, cancellation)?,
                            None => resources.input_scope() != tola_input::InputScope::Pure,
                        }
                    }
                    _ => false,
                };
                if reusable {
                    return Ok(Arc::clone(previous));
                }
            }
            let (collection, verified) = load_remote(config, &original, request)?;
            (
                Arc::new(collection),
                CollectionReads::Remote {
                    locked: verified.locked,
                    source: verified.source,
                },
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
    Ok(Arc::new(LoadedCollection { collection, reads }))
}

fn prepare_directory(
    namespace: &str,
    directory: &Path,
    context: &CollectionInputs<'_>,
    previous: Option<&Arc<LoadedCollection>>,
) -> anyhow::Result<Arc<LoadedCollection>> {
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
    Ok(Arc::new(LoadedCollection {
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
    if tola_input::normalize_existing_prefix(path).starts_with(internal) {
        return Err(IconError::InternalSource {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Configured inputs also remain watchable after a missing or invalid resource aborts a build.
pub(crate) fn configured_collection_watch_evidence(
    config: &SiteConfig,
) -> crate::filesystem::WatchPaths {
    use tola_input::FilesystemSourceIdentity;

    use crate::filesystem::{FilesystemSourceKind, WatchPaths};
    WatchPaths::from_sources(config.icons.collections.values().filter_map(|source| {
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

    #[test]
    fn remote_collections_lock_and_invalidate() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use tola_input::{IconSource, InputScope, Sha256Digest};

        let directory = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/icons.json", listener.local_addr().unwrap());
        let bytes = br#"{"prefix":"brand","icons":{"mark":{"body":"<path d=\"M0 0h24v24H0z\"/>"}},"width":24,"height":24}"#;
        let changed = br#"{"prefix":"brand","icons":{"other":{"body":"<path d=\"M0 0h12v12H0z\"/>"}},"width":24,"height":24}"#;
        let server = std::thread::spawn(move || {
            for body in [bytes.as_slice(), changed.as_slice()] {
                let (mut stream, _) = listener.accept().unwrap();
                assert!(
                    BufReader::new(&stream)
                        .lines()
                        .any(|line| line.unwrap().is_empty())
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let config = crate::config::tests::load_test_config(
            directory.path(),
            &format!(
                "[vendor]\npath = 'vendor'\n[icons.collections.brand]\nsource-type = 'remote-json'\nurl = '{url}'\n"
            ),
        );
        let cancellation = BuildCancellation::default();
        let inventory = prepare_icons(&config, &cancellation, None).unwrap();
        let original = InputLock::read(config.root()).unwrap().unwrap();
        assert_eq!(
            original.icons["brand"].sha256,
            Sha256Digest::of_bytes(bytes)
        );
        assert_eq!(
            original.icons["brand"].source,
            IconSource::Url { url: url.clone() }
        );

        let offline = BuildResources::new()
            .with_input_scope(InputScope::Offline)
            .with_locked(true);
        assert_eq!(
            remote_collection_bytes(&config, "brand", &offline, &cancellation).unwrap(),
            bytes
        );
        let frozen = config.vendor.icon_collection("brand").unwrap();
        std::fs::create_dir_all(frozen.parent().unwrap()).unwrap();
        std::fs::write(&frozen, bytes).unwrap();
        let pure = offline.with_input_scope(InputScope::Pure);
        assert_eq!(
            remote_collection_bytes(&config, "brand", &pure, &cancellation).unwrap(),
            bytes
        );

        let mut updated = original.clone();
        refresh_icon_collections(
            &config,
            &["brand".into()],
            &BuildResources::new(),
            &cancellation,
            &mut updated,
            |_| {},
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(InputLock::read(config.root()).unwrap().unwrap(), original);
        updated.commit(config.root(), &original).unwrap();
        std::fs::write(&frozen, changed).unwrap();
        assert!(
            !inventory
                .evidence
                .is_fresh_for(&config, &cancellation)
                .unwrap()
        );
        let next = prepare(
            &config,
            &cancellation,
            Some(&inventory),
            &pure,
            &SourceOverrides::default(),
        )
        .unwrap();
        assert!(next.collections.get("brand", "other").is_some());
        assert!(next.collections.get("brand", "mark").is_none());
    }

    fn prepare_icons(
        config: &SiteConfig,
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
        const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M0 0h24v24H0z"/></svg>"#;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("brand")).unwrap();
        std::fs::write(root.join("brand/mark.svg"), SVG).unwrap();
        let config = crate::config::tests::load_test_config(
            root,
            r#"
            [icons.collections.brand]
            source-type = "local-svg-dir"
            path = "brand"
        "#,
        );
        let inventory = prepare_icons(&config, &BuildCancellation::new(), None).unwrap();
        symlink(root.join("brand"), root.join(tola_input::INTERNAL_DIR)).unwrap();

        let error =
            prepare_icons(&config, &BuildCancellation::new(), Some(&inventory)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::InternalSource { .. })
        ));
        assert!(inventory.collections().get("brand", "mark").is_some());
    }
}
