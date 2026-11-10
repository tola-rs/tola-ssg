//! Locked remote collection bytes.
//!
//! The lock decides which bytes a build reads. A vendored copy, the site cache, and the network are
//! only ways to obtain those bytes again; none of them may choose a different collection, version,
//! or digest. A resolving build (online, not `--locked`) turns a declaration into a lock entry and
//! records it; every other build verifies the bytes it reads against that entry.

use std::path::{Path, PathBuf};

use crate::cancellation::BuildCancellation;
use crate::filesystem::SourceOverrides;
use crate::resources::{BuildResources, HttpDownloadError};
use tola_input::{IconSource, InputMode, LockedIcon, Sha256Digest};

use super::iconify;
use super::input::{IconFileRead, MAX_COLLECTION_BYTES, read_bounded, read_file};
use super::{IconError, IconIntegrityError};

/// One remote collection request and the context every read in this build shares.
pub(super) struct RemoteCollection<'a> {
    pub(super) namespace: &'a str,
    pub(super) preset: Option<&'a str>,
    pub(super) version: Option<&'a str>,
    pub(super) url: Option<&'a str>,
    pub(super) mirrors: &'a [String],
    /// The lock entry the declaration resolved to, when the site has one.
    pub(super) locked: Option<LockedIcon>,
    pub(super) vendored: Option<PathBuf>,
    pub(super) cache_root: PathBuf,
    pub(super) mode: InputMode,
    pub(super) cancellation: &'a BuildCancellation,
    pub(super) resources: &'a BuildResources,
    pub(super) boundary: &'a tola_typst::SourceBoundary,
}

/// The verified bytes of one remote collection, and where they came from.
pub(super) struct CollectionBytes {
    pub(super) locked: LockedIcon,
    pub(super) bytes: Vec<u8>,
    pub(super) read_from: Option<PathBuf>,
    pub(super) source: Option<IconFileRead>,
    /// The cache entry a freshly downloaded collection belongs in once it has parsed.
    pending_cache: Option<PathBuf>,
}

pub(super) fn load_collection(
    request: RemoteCollection<'_>,
) -> Result<(tola_icons::IconCollection, CollectionBytes), IconError> {
    let verified = collection_bytes(&request)?;
    let collection = super::parse_collection(
        request.namespace,
        verified.read_from.as_deref(),
        &verified.bytes,
    )?;
    store_downloaded(request.namespace, &verified);
    Ok((collection, verified))
}

/// Keep a downloaded collection, which the caller has already parsed, without failing over it.
pub(super) fn store_downloaded(namespace: &str, verified: &CollectionBytes) {
    let Some(path) = &verified.pending_cache else {
        return;
    };
    if write_cache(path, &verified.bytes).is_err() {
        tracing::warn!(target: "tola::compile",
            "the downloaded copy of icon collection `{namespace}` could not be cached; this build publishes the downloaded collection");
    }
}

/// Resolve one declaration to bytes: reuse the lock entry, or record a new one.
fn collection_bytes(request: &RemoteCollection<'_>) -> Result<CollectionBytes, IconError> {
    match matched_lock(request)? {
        Some(locked) => locked_bytes(request, locked),
        None => {
            let (bytes, lock) = resolve(request)?;
            let pending_cache = request
                .mode
                .may_read_cache()
                .then(|| cache_entry(&request.cache_root, lock.sha256));
            Ok(CollectionBytes {
                locked: lock,
                bytes,
                read_from: None,
                source: None,
                pending_cache,
            })
        }
    }
}

/// The lock entry this declaration still describes, if any.
///
/// A build that may not write the lock may not resolve a new entry either: a missing or changed
/// entry is then the author's repair, never a silent re-resolution.
fn matched_lock<'a>(
    request: &'a RemoteCollection<'a>,
) -> Result<Option<&'a LockedIcon>, IconError> {
    let Some(locked) = request.locked.as_ref() else {
        if request.mode.requires_lock() {
            return Err(IconError::Unlocked {
                namespace: request.namespace.into(),
            });
        }
        return Ok(None);
    };
    if declaration_matches(request, locked) {
        return Ok(Some(locked));
    }
    Err(IconError::LockMismatch {
        namespace: request.namespace.into(),
        recorded: describe_lock(locked),
        declared: describe_declaration(request),
    })
}

/// Whether the declaration still asks for exactly the input the lock records.
fn declaration_matches(request: &RemoteCollection<'_>, locked: &LockedIcon) -> bool {
    match (&locked.source, request.preset, request.url) {
        (IconSource::Preset { preset, version }, Some(declared), None) => {
            declared == preset.as_str()
                && request
                    .version
                    .is_none_or(|declared| declared == version.as_str())
        }
        (IconSource::Url { url }, None, Some(declared)) => declared == url,
        _ => false,
    }
}

/// A declaration and its lock entry, for a diagnostic that names both.
fn describe_declaration(request: &RemoteCollection<'_>) -> String {
    match (request.preset, request.url) {
        (Some(preset), None) => match request.version {
            Some(version) => format!("preset `{preset}` release `{version}`"),
            None => format!("preset `{preset}`"),
        },
        (None, Some(url)) => format!("url `{url}`"),
        _ => "no remote source".to_owned(),
    }
}

fn describe_lock(locked: &LockedIcon) -> String {
    match &locked.source {
        IconSource::Preset { preset, version } => {
            format!("preset `{preset}` release `{version}`")
        }
        IconSource::Url { url } => format!("url `{url}`"),
    }
}

/// Read the bytes the lock records: vendored copy first, then the site cache, then the network.
fn locked_bytes(
    request: &RemoteCollection<'_>,
    locked: &LockedIcon,
) -> Result<CollectionBytes, IconError> {
    if let Some(vendored) = request.vendored.as_deref()
        && let Some((bytes, read)) = read_if_present(request, vendored)?
    {
        verify_locked(request, locked, &bytes, Some(vendored))?;
        return Ok(CollectionBytes {
            locked: locked.clone(),
            bytes,
            read_from: Some(vendored.to_path_buf()),
            source: Some(read),
            pending_cache: None,
        });
    }
    if request.mode.may_read_cache() {
        let cached = cache_entry(&request.cache_root, locked.sha256);
        if let Some(bytes) = read_cache(request, &cached)? {
            verify_locked(request, locked, &bytes, Some(&cached))?;
            return Ok(CollectionBytes {
                locked: locked.clone(),
                bytes,
                read_from: Some(cached),
                source: None,
                pending_cache: None,
            });
        }
    }
    if !request.mode.may_use_network() {
        return Err(IconError::MissingBytes {
            namespace: request.namespace.into(),
            sha256: locked.sha256,
        });
    }
    let bytes = fetch_locked(request, locked)?;
    verify_locked(request, locked, &bytes, None)?;
    Ok(CollectionBytes {
        locked: locked.clone(),
        bytes,
        read_from: None,
        source: None,
        pending_cache: request
            .mode
            .may_read_cache()
            .then(|| cache_entry(&request.cache_root, locked.sha256)),
    })
}

/// Turn one declaration into bytes and the lock entry that records them.
fn resolve(request: &RemoteCollection<'_>) -> Result<(Vec<u8>, LockedIcon), IconError> {
    match (request.preset, request.url) {
        (Some(preset), None) => resolve_preset(request, preset),
        (None, Some(url)) => resolve_url(request, url),
        _ => Err(IconError::NotRemote {
            namespace: request.namespace.into(),
        }),
    }
}

/// Resolve one preset through the npm registry's release metadata.
fn resolve_preset(
    request: &RemoteCollection<'_>,
    preset: &str,
) -> Result<(Vec<u8>, LockedIcon), IconError> {
    let release = iconify::release(
        request.namespace,
        preset,
        request.version,
        request.resources,
        request.cancellation,
    )?;
    let mut urls: Vec<String> = request
        .mirrors
        .iter()
        .map(|template| fill_mirror(template, &release.preset, &release.version))
        .collect();
    urls.push(release.tarball.clone());
    let (tarball, _) = download_first(request, &urls, iconify::tarball_limit())?;
    release.verify(request.namespace, &tarball)?;
    let json = release.collection_json(request.namespace, &tarball)?;
    let lock = LockedIcon {
        source: IconSource::Preset {
            preset: release.preset.clone(),
            version: release.version.clone(),
        },
        url: release.tarball,
        sha256: Sha256Digest::of_bytes(&json),
        size: json.len() as u64,
    };
    Ok((json, lock))
}

/// Resolve one authored address. Its first download is trusted; the lock records its digest.
fn resolve_url(
    request: &RemoteCollection<'_>,
    url: &str,
) -> Result<(Vec<u8>, LockedIcon), IconError> {
    let mut urls: Vec<String> = request.mirrors.to_vec();
    urls.push(url.to_owned());
    let (bytes, served) = download_first(request, &urls, MAX_COLLECTION_BYTES)?;
    let lock = LockedIcon {
        source: IconSource::Url {
            url: url.to_owned(),
        },
        url: served,
        sha256: Sha256Digest::of_bytes(&bytes),
        size: bytes.len() as u64,
    };
    Ok((bytes, lock))
}

/// Fetch the bytes one existing lock entry records.
fn fetch_locked(request: &RemoteCollection<'_>, locked: &LockedIcon) -> Result<Vec<u8>, IconError> {
    match &locked.source {
        IconSource::Preset { preset, version } => {
            let mut urls = vec![locked.url.clone()];
            urls.extend(
                request
                    .mirrors
                    .iter()
                    .map(|template| fill_mirror(template, preset, version)),
            );
            let (tarball, _) = download_first(request, &urls, iconify::tarball_limit())?;
            iconify::collection_json(request.namespace, preset, version, &tarball)
        }
        IconSource::Url { url } => {
            let mut urls = vec![locked.url.clone()];
            if !urls.contains(url) {
                urls.push(url.clone());
            }
            urls.extend(request.mirrors.iter().cloned());
            let (bytes, _) = download_first(request, &urls, MAX_COLLECTION_BYTES)?;
            Ok(bytes)
        }
    }
}

/// The addresses in the order first written, so a mirror repeated by hand is visited once.
fn distinct(urls: Vec<String>) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    for url in urls {
        if !ordered.contains(&url) {
            ordered.push(url);
        }
    }
    ordered
}

/// The address one authored mirror template names for this release.
fn fill_mirror(template: &str, name: &str, version: &str) -> String {
    template
        .replace("{name}", name)
        .replace("{version}", version)
}

/// Download the first address that delivers, and say which one did.
fn download_first(
    request: &RemoteCollection<'_>,
    urls: &[String],
    limit: usize,
) -> Result<(Vec<u8>, String), IconError> {
    let mut tried = Vec::new();
    let mut refused = None;
    for url in distinct(urls.to_vec()) {
        request
            .cancellation
            .ensure_active()
            .map_err(IconError::Cancelled)?;
        tried.push(url.clone());
        match download(request, &url, limit) {
            Ok(bytes) => return Ok((bytes, url)),
            // A mirror that cannot deliver the bytes says nothing about the next one.
            Err(error @ IconError::Remote { .. }) => refused = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(all_mirrors_failed(request.namespace, tried, refused))
}

/// Whether a failed download says the address could not deliver.
fn all_mirrors_failed(namespace: &str, urls: Vec<String>, last: Option<IconError>) -> IconError {
    match last {
        Some(IconError::Remote {
            namespace: _,
            urls: _,
            source,
        }) => IconError::Remote {
            namespace: namespace.into(),
            urls,
            source,
        },
        Some(other) => other,
        None => IconError::NetworkWorker,
    }
}

fn download(request: &RemoteCollection<'_>, url: &str, limit: usize) -> Result<Vec<u8>, IconError> {
    request
        .resources
        .download(url, limit, request.cancellation)
        .map_err(|error| match error {
            HttpDownloadError::Cancelled(cancelled) => IconError::Cancelled(cancelled),
            HttpDownloadError::NetworkDenied => IconError::NetworkDenied {
                namespace: request.namespace.into(),
                url: url.into(),
            },
            HttpDownloadError::Runtime(source) => IconError::NetworkRuntime(source),
            HttpDownloadError::Request(source) => IconError::Remote {
                namespace: request.namespace.into(),
                urls: vec![url.into()],
                source: HttpDownloadError::Request(source),
            },
            HttpDownloadError::TooLarge { limit } => IconError::RemoteTooLarge {
                namespace: request.namespace.into(),
                url: url.into(),
                limit,
            },
            HttpDownloadError::WorkerStopped => IconError::NetworkWorker,
        })
}

/// Check bytes against the digest and length the lock records.
fn verify_locked(
    request: &RemoteCollection<'_>,
    locked: &LockedIcon,
    bytes: &[u8],
    path: Option<&Path>,
) -> Result<(), IconError> {
    let received = Sha256Digest::of_bytes(bytes);
    if received == locked.sha256 && bytes.len() as u64 == locked.size {
        return Ok(());
    }
    Err(IconError::Integrity(Box::new(IconIntegrityError {
        namespace: request.namespace.into(),
        url: locked.url.clone(),
        expected: locked.sha256.to_string(),
        received: received.to_string(),
        size_expected: locked.size,
        size_received: bytes.len() as u64,
        path: path.map(Path::to_path_buf),
    })))
}

/// The cache entry one digest is stored in.
fn cache_entry(cache_root: &Path, sha256: Sha256Digest) -> PathBuf {
    cache_root.join(format!("{sha256}.json"))
}

/// Read one source file, treating a missing file as no copy at all.
fn read_if_present(
    request: &RemoteCollection<'_>,
    path: &Path,
) -> Result<Option<(Vec<u8>, IconFileRead)>, IconError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(IconError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(IconError::Read {
                path: path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "an icon collection source is not a regular file",
                ),
            });
        }
        Ok(_) => {}
    }
    let overrides = SourceOverrides::default();
    let (bytes, read) = read_file(path, &overrides, request.cancellation, request.boundary)?;
    Ok(Some((bytes.into_owned(), read)))
}

/// Read one verified cache entry; the cache is not a source and is not watched.
fn read_cache(request: &RemoteCollection<'_>, path: &Path) -> Result<Option<Vec<u8>>, IconError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(IconError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(IconError::Read {
                path: path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "an icon cache entry is not a regular file",
                ),
            });
        }
        Ok(_) => {}
    }
    let mut file = tola_fs::open_regular_file(path, false).map_err(|source| IconError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    read_bounded(&mut file, path, request.cancellation).map(Some)
}

fn write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .expect("icon cache entries have a parent directory");
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
