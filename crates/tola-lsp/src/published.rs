//! Published packages for one compiler connection, never a process-global cache.

use std::sync::Arc;

use serde::Deserialize;
use tola_build::{BuildResources, NetworkAccess};
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
    pub(super) fn snapshot(
        &mut self,
        resources: &BuildResources,
        locations: &PackageLocations,
        requested: bool,
    ) -> &[PublishedPackage] {
        // A connection must not advertise packages its build cannot obtain.
        if resources.network_access() == NetworkAccess::Denied || locations.cache().is_none() {
            return &[];
        }
        #[cfg(feature = "network")]
        if requested {
            self.poll(resources, "https://packages.typst.org/preview/index.json");
        }
        #[cfg(not(feature = "network"))]
        let _ = requested;
        &self.packages
    }

    #[cfg(feature = "network")]
    fn poll(&mut self, resources: &BuildResources, url: &str) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            match self.worker.take().expect("completed index worker").join() {
                Ok(Some(packages)) => self.packages = packages,
                Ok(None) => {}
                Err(_) => tracing::debug!("published package index worker stopped"),
            }
        }
        if self.started || resources.network_access() == NetworkAccess::Denied {
            return;
        }
        self.started = true;
        let resources = resources.clone();
        let cancellation = self.canceller.token();
        let url = url.to_owned();
        match std::thread::Builder::new()
            .name("tola-lsp-packages".into())
            .spawn(move || download(&resources, &url, &cancellation))
        {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => tracing::debug!(%error, "published package index worker could not start"),
        }
    }
}

#[cfg(feature = "network")]
impl Drop for PublishedIndex {
    fn drop(&mut self) {
        self.canceller.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(any(feature = "network", test))]
fn parse_index(index: &[u8]) -> Option<Arc<[PublishedPackage]>> {
    serde_json::from_slice::<Vec<PublishedPackage>>(index)
        .ok()
        .map(Arc::from)
}

#[cfg(feature = "network")]
fn download(
    resources: &BuildResources,
    url: &str,
    cancellation: &BuildCancellation,
) -> Option<Arc<[PublishedPackage]>> {
    const INDEX_LIMIT: usize = 16 * 1024 * 1024;
    let bytes = resources
        .download(url, INDEX_LIMIT, cancellation)
        .map_err(|error| tracing::debug!(%error, "published package index is unavailable"))
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
    fn malformed_index_is_unavailable() {
        for bytes in [
            b"<html>maintenance</html>".as_slice(),
            b"[{\"name\": \"cetz\"}]",
        ] {
            assert!(parse_index(bytes).is_none());
        }
    }

    #[test]
    fn packages_empty_without_network_access_or_cache() {
        let directory = tempfile::tempdir().unwrap();
        let index_bytes = br#"[{"name":"remote-only","version":"1.0.0"}]"#;
        for (scope, cache) in [
            (tola_build::InputScope::Online, None),
            (
                tola_build::InputScope::Offline,
                Some(directory.path().to_path_buf()),
            ),
            (
                tola_build::InputScope::Pure,
                Some(directory.path().to_path_buf()),
            ),
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
            let resources = BuildResources::new().with_input_scope(scope);
            let locations = PackageLocations::from_absolute_roots(None, cache).unwrap();
            assert!(index.snapshot(&resources, &locations, false).is_empty());
        }
    }

    #[cfg(feature = "network")]
    #[test]
    fn restricted_index_never_connects() {
        for scope in [
            tola_build::InputScope::Offline,
            tola_build::InputScope::Pure,
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/index.json", listener.local_addr().unwrap());
            let resources = BuildResources::new().with_input_scope(scope);
            let mut index = PublishedIndex::default();
            index.poll(&resources, &url);
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
