//! Per-connection published package index, never a process-global cache.

use std::sync::Arc;

use serde::Deserialize;
use tola_build::BuildResources;
use tola_input::NetworkAccess;
use tola_typst::PackageLocations;

#[cfg(feature = "network")]
use tola_build::cancellation::{BuildCancellation, BuildCanceller};

#[derive(Debug, Deserialize)]
pub(super) struct PublishedPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
}

#[derive(Default)]
pub(super) struct PublishedIndex {
    packages: Arc<[PublishedPackage]>,
    #[cfg(feature = "network")]
    worker: Option<std::thread::JoinHandle<Option<Arc<[PublishedPackage]>>>>,
    #[cfg(feature = "network")]
    canceller: BuildCanceller,
    #[cfg(feature = "network")]
    started: bool,
}

impl PublishedIndex {
    pub(super) fn published_packages(
        &mut self,
        res: &BuildResources,
        locs: &PackageLocations,
        requested: bool,
    ) -> &[PublishedPackage] {
        // Never advertise packages the connection's build policy cannot obtain.
        if res.network_access() == NetworkAccess::Denied || locs.cache().is_none() {
            return &[];
        }
        #[cfg(feature = "network")]
        if requested {
            self.poll(res, "https://packages.typst.org/preview/index.json");
        }
        #[cfg(not(feature = "network"))]
        let _ = requested;
        &self.packages
    }

    #[cfg(feature = "network")]
    fn poll(&mut self, res: &BuildResources, url: &str) {
        if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
            match self.worker.take().expect("completed index worker").join() {
                Ok(Some(pkgs)) => self.packages = pkgs,
                Ok(None) => {}
                Err(_) => tracing::debug!("published package index worker stopped"),
            }
        }
        if self.started || res.network_access() == NetworkAccess::Denied {
            return;
        }
        self.started = true;
        let res = res.clone();
        let cancel = self.canceller.token();
        let url = url.to_owned();
        match std::thread::Builder::new()
            .name("tola-lsp-packages".into())
            .spawn(move || download(&res, &url, &cancel))
        {
            Ok(w) => self.worker = Some(w),
            Err(err) => tracing::debug!(%err, "published package index worker could not start"),
        }
    }
}

#[cfg(feature = "network")]
impl Drop for PublishedIndex {
    fn drop(&mut self) {
        self.canceller.cancel();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(any(feature = "network", test))]
fn parse_index(raw: &[u8]) -> Option<Arc<[PublishedPackage]>> {
    serde_json::from_slice::<Vec<PublishedPackage>>(raw)
        .ok()
        .map(Arc::from)
}

#[cfg(feature = "network")]
fn download(
    res: &BuildResources,
    url: &str,
    cancel: &BuildCancellation,
) -> Option<Arc<[PublishedPackage]>> {
    const INDEX_LIMIT: usize = 16 * 1024 * 1024;
    let bytes = res
        .download(url, INDEX_LIMIT, cancel)
        .map_err(|err| tracing::debug!(%err, "published package index is unavailable"))
        .ok()?;
    parse_index(&bytes).or_else(|| {
        tracing::debug!("published package index did not parse");
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_require_network_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let index_bytes = br#"[{"name":"remote-only","version":"1.0.0"}]"#;
        for (scope, cache) in [
            (tola_input::InputScope::Online, None),
            (
                tola_input::InputScope::Offline,
                Some(dir.path().to_path_buf()),
            ),
            (tola_input::InputScope::Pure, Some(dir.path().to_path_buf())),
        ] {
            let mut index = PublishedIndex {
                packages: parse_index(index_bytes).unwrap(),
                #[cfg(feature = "network")]
                worker: None,
                #[cfg(feature = "network")]
                canceller: BuildCanceller::new(),
                #[cfg(feature = "network")]
                started: false,
            };
            let res = BuildResources::new().with_input_scope(scope);
            let locs = PackageLocations::from_absolute_roots(None, cache).unwrap();
            assert!(index.published_packages(&res, &locs, false).is_empty());
        }
    }

    #[cfg(feature = "network")]
    #[test]
    fn restricted_index_never_connects() {
        for scope in [
            tola_input::InputScope::Offline,
            tola_input::InputScope::Pure,
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/index.json", listener.local_addr().unwrap());
            let res = BuildResources::new().with_input_scope(scope);
            let mut index = PublishedIndex::default();
            index.poll(&res, &url);
            drop(index);
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[cfg(feature = "network")]
    #[test]
    fn closing_index_cancels_download() {
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/index.json", listener.local_addr().unwrap());
        let (accepted, socket) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            accepted.send(listener.accept().unwrap().0).unwrap();
        });
        let mut index = PublishedIndex::default();
        index.poll(&BuildResources::new(), &url);
        let _socket = socket.recv_timeout(Duration::from_secs(5)).unwrap();
        let (closed, closing) = mpsc::sync_channel(1);
        let client = std::thread::spawn(move || {
            drop(index);
            closed.send(()).unwrap();
        });
        closing
            .recv_timeout(Duration::from_secs(5))
            .expect("connection cancellation ends a stalled index read");
        client.join().unwrap();
        server.join().unwrap();
    }
}
