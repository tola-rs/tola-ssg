//! Integrity-addressed remote collection bytes; cache entries never select a version.

use std::io::Write;
use std::path::Path;

use anyhow::Context;

use sha2::{Digest, Sha256};

use crate::cancellation::BuildCancellation;
use crate::config::section::Sha256Digest;
use crate::resources::{BuildResources, HttpDownloadError};

use super::IconError;
use super::input::MAX_COLLECTION_BYTES;

/// One remote collection request: which collection, where its copies may come from, and the
/// request context every read in this build shares.
pub(super) struct RemoteCollection<'a> {
    pub namespace: &'a str,
    pub urls: &'a [String],
    pub sha256: Sha256Digest,
    pub vendored: Option<&'a Path>,
    pub cache_root: &'a Path,
    pub cancellation: &'a BuildCancellation,
    pub resources: &'a BuildResources,
    pub boundary: &'a tola_typst::SourceBoundary,
}

pub(super) fn load_collection(
    request: RemoteCollection<'_>,
) -> anyhow::Result<(
    tola_icons::IconCollection,
    Option<super::input::IconFileRead>,
)> {
    let verified = collection_bytes(&request)?;
    let namespace = request.namespace;
    let collection =
        super::parse_collection(namespace, verified.read_from.as_deref(), &verified.bytes)
            .map_err(anyhow::Error::from)?;
    store_downloaded(namespace, &verified);
    Ok((collection, verified.source))
}

/// The bytes of one remote collection, and where they came from.
pub(super) struct CollectionBytes {
    pub(super) bytes: Vec<u8>,
    pub(super) read_from: Option<std::path::PathBuf>,
    source: Option<super::input::IconFileRead>,
    /// The cache entry a freshly downloaded collection belongs in once it has parsed.
    pending_cache: Option<std::path::PathBuf>,
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

/// The bytes one remote collection resolves to, checked against the digest it must have.
///
/// The site's vendored copy is read first, then the collection cache, then the collection itself,
/// trying each mirror in turn when one cannot deliver. Every source answers with the same bytes,
/// because the digest decides which bytes count, so reusing one is never a second semantic path.
pub(super) fn collection_bytes(request: &RemoteCollection<'_>) -> anyhow::Result<CollectionBytes> {
    let RemoteCollection {
        namespace,
        urls,
        sha256,
        vendored,
        cache_root,
        cancellation,
        resources,
        boundary,
    } = *request;
    cancellation.ensure_active()?;
    let primary = urls
        .first()
        .map(String::as_str)
        .expect("configuration names at least one address");
    // A copy the site has is authoritative: the site asks for exactly these bytes, so a
    // missing one falls back to the cache and the network, and a wrong one fails the build.
    if let Some(vendored) = vendored {
        boundary.check(vendored)?;
        let bytes =
            read_collection_file(vendored, cancellation, Some(boundary)).with_context(|| {
                format!("could not read the vendored copy of icon collection `{namespace}`")
            })?;
        if let Some((bytes, source)) = bytes {
            verify_integrity(namespace, primary, sha256, &bytes, Some(vendored))?;
            return Ok(CollectionBytes {
                bytes,
                read_from: Some(vendored.to_path_buf()),
                pending_cache: None,
                source,
            });
        }
    }

    let cache_path = cache_entry(cache_root, sha256);
    if resources.input_scope() != crate::InputScope::Pure {
        match read_collection_file(&cache_path, cancellation, None) {
            Ok(Some((bytes, _))) => {
                match verify_integrity(namespace, primary, sha256, &bytes, Some(&cache_path)) {
                    Ok(()) => {
                        return Ok(CollectionBytes {
                            bytes,
                            read_from: Some(cache_path),
                            pending_cache: None,
                            source: None,
                        });
                    }
                    Err(_) => tracing::warn!(target: "tola::compile",
                    "the cached copy of icon collection `{namespace}` could not be used; Tola will download it again"),
                }
            }
            Ok(None) => {}
            Err(_) => {
                cancellation.ensure_active()?;
                tracing::warn!(target: "tola::compile",
                    "the cached copy of icon collection `{namespace}` could not be read; Tola will download it again");
            }
        }
    }

    let mut tried: Vec<String> = Vec::new();
    let mut unreachable = None;
    for url in urls {
        cancellation.ensure_active()?;
        tried.push(url.clone());
        match download(namespace, url, cancellation, resources) {
            Ok(bytes) => {
                verify_integrity(namespace, url, sha256, &bytes, None)?;
                cancellation.ensure_active()?;
                return Ok(CollectionBytes {
                    bytes,
                    read_from: None,
                    pending_cache: Some(cache_path),
                    source: None,
                });
            }
            // A mirror that cannot deliver the bytes says nothing about the next one.
            Err(error) if refused(&error) => unreachable = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(all_mirrors_failed(namespace, tried, unreachable).into())
}

/// The cache entry one digest is stored in.
fn cache_entry(cache_root: &Path, sha256: Sha256Digest) -> std::path::PathBuf {
    cache_root.join(format!("{sha256}.json"))
}

fn verify_integrity(
    namespace: &str,
    url: &str,
    expected: Sha256Digest,
    bytes: &[u8],
    path: Option<&Path>,
) -> Result<(), IconError> {
    let received: [u8; 32] = Sha256::digest(bytes).into();
    if expected.bytes() != received {
        return Err(IconError::Integrity {
            namespace: namespace.into(),
            url: url.into(),
            expected: expected.to_string(),
            received: hex::encode(received),
            path: path.map(Path::to_path_buf),
        });
    }
    Ok(())
}

fn read_collection_file(
    path: &Path,
    cancellation: &BuildCancellation,
    boundary: Option<&tola_typst::SourceBoundary>,
) -> anyhow::Result<Option<(Vec<u8>, Option<super::input::IconFileRead>)>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "an icon collection source is not a regular file",
            )
            .into());
        }
        Ok(_) => {}
    }
    if let Some(boundary) = boundary {
        let overrides = Default::default();
        let (bytes, read) = super::input::read_file(path, &overrides, cancellation, boundary)?;
        Ok(Some((bytes.into_owned(), Some(read))))
    } else {
        let mut file = crate::filesystem::open_regular_file(path, false)?;
        let bytes = super::input::read_bounded(&mut file, path, cancellation)?;
        Ok(Some((bytes, None)))
    }
}

fn write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

/// Whether a failed download says the address could not deliver, rather than the build cannot ask.
///
/// Only a transport or status failure is worth handing to the next mirror: a denied network, an
/// oversized collection, and a cancelled build answer the same way at every address.
fn refused(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<IconError>(),
        Some(IconError::Remote { .. })
    )
}

/// The failure every mirror shares, carrying the last transport error.
fn all_mirrors_failed(
    namespace: &str,
    urls: Vec<String>,
    last: Option<anyhow::Error>,
) -> IconError {
    let source = last
        .and_then(|error| match error.downcast::<IconError>() {
            Ok(IconError::Remote { source, .. }) => Some(source),
            _ => None,
        })
        .expect("a refused download carries its transport error");
    IconError::Remote {
        namespace: namespace.into(),
        urls,
        source,
    }
}

fn download(
    namespace: &str,
    url: &str,
    cancellation: &BuildCancellation,
    resources: &BuildResources,
) -> anyhow::Result<Vec<u8>> {
    resources
        .download(url, MAX_COLLECTION_BYTES, cancellation)
        .map_err(|error| match error {
            HttpDownloadError::Cancelled(cancelled) => anyhow::Error::new(cancelled),
            HttpDownloadError::NetworkDenied => IconError::NetworkDenied {
                namespace: namespace.into(),
                url: url.into(),
            }
            .into(),
            HttpDownloadError::Runtime(error) => IconError::NetworkRuntime(error).into(),
            HttpDownloadError::Request(source) => IconError::Remote {
                namespace: namespace.into(),
                urls: vec![url.into()],
                source,
            }
            .into(),
            HttpDownloadError::TooLarge { limit } => IconError::RemoteTooLarge {
                namespace: namespace.into(),
                url: url.into(),
                limit,
            }
            .into(),
            HttpDownloadError::WorkerStopped => IconError::NetworkWorker.into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::NetworkAccess;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    const COLLECTION: &[u8] = br#"{"prefix":"ui","icons":{"mark":{"body":"<path fill=\"currentColor\" d=\"M0 0h16v16H0z\"/>"}}}"#;

    fn digest(bytes: &[u8]) -> Sha256Digest {
        hex::encode(Sha256::digest(bytes)).try_into().unwrap()
    }

    fn load_offline(
        cache_root: &Path,
        sha256: Sha256Digest,
        vendored: Option<&Path>,
    ) -> anyhow::Result<tola_icons::IconCollection> {
        load_collection(RemoteCollection {
            namespace: "ui",
            urls: &["http://127.0.0.1:1/icons.json".to_owned()],
            sha256,
            vendored,
            cache_root,
            cancellation: &BuildCancellation::new(),
            resources: &BuildResources::new().with_network_access(NetworkAccess::Denied),
            boundary: &tola_typst::SourceBoundary::default(),
        })
        .map(|(collection, _)| collection)
    }

    fn load_online(
        cache_root: &Path,
        url: &str,
        sha256: Sha256Digest,
    ) -> anyhow::Result<tola_icons::IconCollection> {
        load_mirrors(cache_root, &[url.to_owned()], sha256)
    }

    fn load_mirrors(
        cache_root: &Path,
        urls: &[String],
        sha256: Sha256Digest,
    ) -> anyhow::Result<tola_icons::IconCollection> {
        load_collection(RemoteCollection {
            namespace: "ui",
            urls,
            sha256,
            vendored: None,
            cache_root,
            cancellation: &BuildCancellation::new(),
            resources: &BuildResources::new(),
            boundary: &tola_typst::SourceBoundary::default(),
        })
        .map(|(collection, _)| collection)
    }

    #[test]
    fn cached_icons_do_not_satisfy_pure() {
        let directory = tempfile::tempdir().unwrap();
        let sha256 = digest(COLLECTION);
        let config = crate::config::tests::load_test_config(
            directory.path(),
            &format!(
                "[icons.collections.ui]\nsource-type = \"remote-json\"\nurl = \"http://127.0.0.1:1/icons.json\"\nsha256 = \"{sha256}\"\n"
            ),
        );
        let cache = BuildResources::icon_cache_directory(config.get_root());
        write_cache(&cache.join(format!("{sha256}.json")), COLLECTION).unwrap();
        let cancellation = BuildCancellation::new();
        let offline = BuildResources::new().with_input_scope(crate::InputScope::Offline);
        let previous =
            super::super::prepare(&config, &cancellation, None, &offline, &Default::default())
                .unwrap();
        assert!(previous.collections().get("ui", "mark").is_some());
        let pure = offline.with_input_scope(crate::InputScope::Pure);
        let error = super::super::prepare(
            &config,
            &cancellation,
            Some(&previous),
            &pure,
            &Default::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::NetworkDenied { .. })
        ));
    }

    #[test]
    fn removing_vendor_invalidates_pure_icons() {
        let directory = tempfile::tempdir().unwrap();
        let sha256 = digest(COLLECTION);
        let config = crate::config::tests::load_test_config(
            directory.path(),
            &format!(
                "[vendor]\npath = \"vendor\"\n[icons.collections.ui]\nsource-type = \"remote-json\"\nurl = \"http://127.0.0.1:1/icons.json\"\nsha256 = \"{sha256}\"\n"
            ),
        );
        let vendored = config.vendor.icon_collection("ui").unwrap();
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, COLLECTION).unwrap();
        let cancellation = BuildCancellation::new();
        let pure = BuildResources::new().with_input_scope(crate::InputScope::Pure);
        let previous =
            super::super::prepare(&config, &cancellation, None, &pure, &Default::default())
                .unwrap();
        assert!(previous.collections().get("ui", "mark").is_some());
        std::fs::remove_file(&vendored).unwrap();
        assert!(!previous.evidence().is_fresh(&cancellation).unwrap());
        assert!(
            super::super::prepare(
                &config,
                &cancellation,
                Some(&previous),
                &pure,
                &Default::default(),
            )
            .is_err()
        );
    }

    fn read_request_headers(stream: &mut std::net::TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0, "request ended before its headers");
            headers.extend_from_slice(&chunk[..read]);
            if headers.windows(4).any(|ending| ending == b"\r\n\r\n") {
                return;
            }
            assert!(
                headers.len() <= 16 * 1024,
                "unexpectedly large test request"
            );
        }
    }

    fn serve_collection(bytes: &'static [u8]) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/icons.json", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_headers(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(bytes).unwrap();
        });
        (url, server)
    }

    /// A server that answers every request with one status, and no bytes.
    fn serve_status(status: u16) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/icons.json", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_headers(&mut stream);
            write!(
                stream,
                "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        (url, server)
    }

    #[test]
    fn refused_download_tries_the_next_mirror() {
        let directory = tempfile::tempdir().unwrap();
        let (refused, refused_server) = serve_status(503);
        let (served, served_server) = serve_collection(COLLECTION);
        let collection =
            load_mirrors(directory.path(), &[refused, served], digest(COLLECTION)).unwrap();
        assert!(collection.get("mark").is_some());
        refused_server.join().unwrap();
        served_server.join().unwrap();
    }

    #[test]
    fn every_mirror_refusing_names_all_of_them() {
        let directory = tempfile::tempdir().unwrap();
        let (first, first_server) = serve_status(503);
        let (second, second_server) = serve_status(404);
        let error = load_mirrors(
            directory.path(),
            &[first.clone(), second.clone()],
            digest(COLLECTION),
        )
        .unwrap_err();
        let IconError::Remote { urls, .. } = error
            .downcast_ref::<IconError>()
            .expect("a refused download is a download failure")
        else {
            panic!("expected a download failure")
        };
        assert_eq!(urls, &[first, second]);
        first_server.join().unwrap();
        second_server.join().unwrap();
    }

    #[test]
    fn verified_cache_serves_offline() {
        let directory = tempfile::tempdir().unwrap();
        let sha256 = digest(COLLECTION);
        write_cache(&directory.path().join(format!("{sha256}.json")), COLLECTION).unwrap();
        let collection = load_offline(directory.path(), sha256, None).unwrap();
        assert!(collection.get("mark").is_some());
    }

    #[test]
    fn vendored_copy_serves_without_network() {
        let directory = tempfile::tempdir().unwrap();
        let vendored = directory.path().join("vendor/icons/ui.json");
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, COLLECTION).unwrap();
        let collection =
            load_offline(directory.path(), digest(COLLECTION), Some(&vendored)).unwrap();
        assert!(collection.get("mark").is_some());
    }

    #[test]
    fn stale_vendored_copy_fails_the_build() {
        let directory = tempfile::tempdir().unwrap();
        let vendored = directory.path().join("vendor/icons/ui.json");
        std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
        std::fs::write(&vendored, b"an older collection").unwrap();
        let error =
            load_offline(directory.path(), digest(COLLECTION), Some(&vendored)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::Integrity {
                path: Some(path),
                ..
            }) if path == &vendored
        ));
    }

    #[test]
    fn uncached_offline_source_is_denied() {
        let directory = tempfile::tempdir().unwrap();
        let error = load_offline(directory.path(), digest(COLLECTION), None).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::NetworkDenied { .. })
        ));
    }

    #[test]
    fn digest_mismatch_is_never_cached() {
        let directory = tempfile::tempdir().unwrap();
        let (url, server) = serve_collection(COLLECTION);
        let sha256 = digest(b"different bytes");
        let error = load_online(directory.path(), &url, sha256).unwrap_err();
        server.join().unwrap();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::Integrity { .. })
        ));
        assert!(!directory.path().join(format!("{sha256}.json")).exists());
    }

    #[test]
    fn corrupt_cache_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let sha256 = digest(COLLECTION);
        let cache_path = directory.path().join(format!("{sha256}.json"));
        std::fs::write(&cache_path, "wrong content").unwrap();
        let (url, server) = serve_collection(COLLECTION);
        let collection = load_online(directory.path(), &url, sha256).unwrap();
        server.join().unwrap();
        assert!(collection.get("mark").is_some());
        assert_eq!(std::fs::read(&cache_path).unwrap(), COLLECTION);
    }

    #[test]
    fn pinned_collection_survives_local_edits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let sha256 = digest(COLLECTION);
        let cache = root.join(".tola/cache/icons/sha256");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join(format!("{sha256}.json")), b"corrupt").unwrap();
        std::fs::write(root.join("local.json"), COLLECTION).unwrap();
        let (url, server) = serve_collection(COLLECTION);
        let source = format!(
            r#"
            [icons.collections.remote]
            source-type = "remote-json"
            url = "{url}"
            sha256 = "{sha256}"
            [icons.collections.local]
            source-type = "local-json"
            path = "local.json"
        "#
        );
        let config = crate::config::tests::load_test_config(root, &source);
        let cancellation = BuildCancellation::new();
        let first = super::super::prepare(
            &config,
            &cancellation,
            None,
            &BuildResources::new(),
            &Default::default(),
        )
        .unwrap();
        server.join().unwrap();
        std::fs::write(
            root.join("local.json"),
            br#"{"prefix":"ui","icons":{"mark":{"body":"<circle r='2'/>"}}}"#,
        )
        .unwrap();
        let changed = super::super::prepare(
            &config,
            &cancellation,
            Some(&first),
            &BuildResources::new(),
            &Default::default(),
        )
        .unwrap();
        assert!(std::ptr::eq(
            changed.collections().collection("remote").unwrap(),
            first.collections().collection("remote").unwrap(),
        ));
    }

    #[test]
    fn invalid_collection_is_never_cached() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = b"{invalid JSON";
        let sha256 = digest(bytes);
        let (url, server) = serve_collection(bytes);
        let error = load_online(directory.path(), &url, sha256).unwrap_err();
        server.join().unwrap();
        assert!(matches!(
            error.downcast_ref::<IconError>(),
            Some(IconError::Collection { .. })
        ));
        assert!(!directory.path().join(format!("{sha256}.json")).exists());
    }

    #[test]
    fn cancellation_ends_pending_download() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/icons.json", listener.local_addr().unwrap());
        let (accepted, request) = mpsc::sync_channel(1);
        let (finish, release) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_headers(&mut stream);
            accepted.send(()).unwrap();
            release.recv().unwrap();
        });
        let canceller = crate::cancellation::BuildCanceller::new();
        let worker_cancellation = canceller.token();
        let (completed, completion) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            completed
                .send(download(
                    "ui",
                    &url,
                    &worker_cancellation,
                    &BuildResources::new(),
                ))
                .unwrap();
        });
        request.recv_timeout(Duration::from_secs(5)).unwrap();
        canceller.cancel();
        let outcome = completion.recv_timeout(Duration::from_secs(2));
        finish.send(()).unwrap();
        server.join().unwrap();
        worker.join().unwrap();
        let error = outcome
            .expect("cancelled remote wait did not terminate promptly")
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::cancellation::BuildCancelled>(),
            Some(crate::cancellation::BuildCancelled)
        ));
    }
}
