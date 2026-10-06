//! Host-selected I/O resources shared across build sessions and attempts.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::filesystem::INTERNAL_DIR;
use crate::image::PixelCache;

/// Which original inputs a reusable build may consume. Hooks remain trusted host code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputScope {
    #[default]
    Online,
    Offline,
    Pure,
}

/// Network access for built-in package and remote-icon fetching.
///
/// Does not constrain the external scripts selected by a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkAccess {
    /// Missing resources may be downloaded.
    #[default]
    Allowed,
    /// Only resources already available locally may be used.
    Denied,
}

/// Shared native I/O resources, independent of a session's accepted build results.
///
/// Clones share the parsed-file cache and lazily created HTTP connection pool
/// and runtime. Construction reads and writes no file and opens no connection. Icons are
/// cached under the site's `.tola/cache/icons/sha256` directory.
#[derive(Clone)]
pub struct BuildResources {
    scope: InputScope,
    network: NetworkAccess,
    files: Arc<tola_typst::SharedFileCache>,
    include_system_fonts: bool,
    downloads: Arc<Mutex<Option<Arc<HttpDownloads>>>>,
    pixels: Arc<PixelCache>,
}

impl BuildResources {
    /// Default resources, with network fetching allowed and system fonts included.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict built-in I/O without changing the caller's independent resource preferences.
    /// Restrictions only tighten on an existing handle; clone before restricting to retain another scope.
    pub fn with_input_scope(mut self, scope: InputScope) -> Self {
        self.scope = match (self.scope, scope) {
            (InputScope::Pure, _) | (_, InputScope::Pure) => InputScope::Pure,
            (InputScope::Offline, _) | (_, InputScope::Offline) => InputScope::Offline,
            _ => InputScope::Online,
        };
        self
    }

    pub fn input_scope(&self) -> InputScope {
        self.scope
    }

    /// Effective package tiers, excluding host roots in pure builds even when they are local.
    pub fn package_locations(
        &self,
        config: &crate::config::ResolvedSiteConfig,
    ) -> tola_typst::PackageLocations {
        let locations = config.package_locations().clone();
        if self.scope == InputScope::Pure {
            locations.without_host_roots()
        } else {
            locations
        }
    }

    /// The same physical restrictions used by built-in source readers and editor integrations.
    pub fn source_boundary(
        &self,
        config: &crate::config::ResolvedSiteConfig,
    ) -> tola_typst::SourceBoundary {
        source_boundary(config, self.scope)
    }

    /// Select whether built-in remote resources may be fetched.
    pub fn with_network_access(mut self, access: NetworkAccess) -> Self {
        self.network = access;
        self
    }

    /// Share parsed Typst files with other sessions or native Typst integrations.
    pub fn with_file_cache(mut self, cache: Arc<tola_typst::SharedFileCache>) -> Self {
        self.files = cache;
        self
    }

    /// Effective network access; a restrictive scope cannot be loosened by another builder.
    pub fn network_access(&self) -> NetworkAccess {
        match self.scope {
            InputScope::Online => self.network,
            InputScope::Offline | InputScope::Pure => NetworkAccess::Denied,
        }
    }

    pub(crate) fn file_cache(&self) -> Arc<tola_typst::SharedFileCache> {
        Arc::clone(&self.files)
    }

    /// The pixel planes every session sharing these resources keeps between its requests.
    pub(crate) fn pixel_cache(&self) -> Arc<PixelCache> {
        Arc::clone(&self.pixels)
    }

    /// Where remote icon collections are cached under one site root.
    ///
    /// Offline builds may reuse this machine-dependent copy. Pure builds require a local or
    /// vendored original and never use this cache; deleting it cannot remove a pure build's source.
    pub fn icon_cache_directory(site_root: &Path) -> PathBuf {
        site_root.join(".tola/cache/icons/sha256")
    }

    /// Where encoded image variants are kept under one site root.
    ///
    /// A variant is stored only after its bytes are complete, and read back only when they match
    /// the digest recorded beside them, so removing the directory costs a render and nothing else.
    pub fn image_cache_directory(site_root: &Path) -> PathBuf {
        site_root.join(INTERNAL_DIR).join("cache/images")
    }

    /// Exclude system fonts from a build.
    pub fn without_system_fonts(mut self) -> Self {
        self.include_system_fonts = false;
        self
    }

    /// Whether system fonts may join a build.
    pub fn include_system_fonts(&self) -> bool {
        self.include_system_fonts && self.scope != InputScope::Pure
    }

    /// Download at most `limit` bytes using the host's network policy and shared HTTP pool.
    ///
    /// A denied policy opens no connection. Cancellation interrupts an in-flight request.
    pub fn download(
        &self,
        url: &str,
        limit: usize,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<u8>, HttpDownloadError> {
        cancellation.ensure_active()?;
        if self.network_access() == NetworkAccess::Denied {
            return Err(HttpDownloadError::NetworkDenied);
        }
        let downloads = self.http_downloads()?;
        downloads.download(url, limit, cancellation)
    }

    fn http_downloads(&self) -> Result<Arc<HttpDownloads>, HttpDownloadError> {
        let mut downloads = self
            .downloads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(downloads) = &*downloads {
            return Ok(Arc::clone(downloads));
        }
        let prepared = Arc::new(HttpDownloads::new()?);
        *downloads = Some(Arc::clone(&prepared));
        Ok(prepared)
    }
}

/// The exclusions every site read applies, before any output path is known.
///
/// Configuration is read before a resolved configuration exists, so it shares this much of the
/// boundary and adds the output exclusions later through [`source_boundary`]. The package view
/// below the internal directory is the one exception: `tola editor setup` publishes a site's
/// packages there, and a definition into a package names those files, so a file-only client reads
/// them as sources while every other generated path stays refused.
pub(crate) fn base_source_boundary(root: &Path, scope: InputScope) -> tola_typst::SourceBoundary {
    tola_typst::SourceBoundary::new(root, scope == InputScope::Pure)
        .excluding_except(
            root.join(INTERNAL_DIR),
            root.join(crate::filesystem::PACKAGE_MIRROR_DIRECTORY),
        )
        .excluding(root.join(crate::filesystem::SITE_BUILD_LOCK_FILE))
}

pub(crate) fn source_boundary(
    config: &crate::config::ResolvedSiteConfig,
    scope: InputScope,
) -> tola_typst::SourceBoundary {
    let root = config.get_root();
    let mut boundary = base_source_boundary(root, scope);
    // The output tree and the publication workspace beside it hold bytes Tola wrote, not inputs: a
    // read that reaches either would compile staged or stale output. Publication still resolves the
    // output root against its protected inputs, because that check owns the output layout rather
    // than read permission. A vendor candidate is granted back: it is the input being prepared.
    for exclusion in config.generated_exclusions() {
        boundary = match exclusion.candidate {
            Some(candidate) => boundary.excluding_except(exclusion.directory, candidate),
            None => boundary.excluding(exclusion.directory),
        };
    }
    boundary
}

impl Default for BuildResources {
    fn default() -> Self {
        Self {
            scope: InputScope::Online,
            network: NetworkAccess::Allowed,
            files: Arc::new(tola_typst::SharedFileCache::new()),
            include_system_fonts: true,
            downloads: Arc::new(Mutex::new(None)),
            pixels: Arc::new(PixelCache::default()),
        }
    }
}

/// A remote resource could not be downloaded under the host's resource limits.
#[derive(Debug, thiserror::Error)]
pub enum HttpDownloadError {
    /// The host forbids network access.
    #[error("network access is disabled")]
    NetworkDenied,
    /// The host could not prepare asynchronous I/O.
    #[error("failed to prepare HTTP runtime: {0}")]
    Runtime(#[source] std::io::Error),
    /// The HTTP request or response failed.
    #[error("{0}")]
    Request(#[source] reqwest::Error),
    /// The response exceeded the caller's byte limit.
    #[error("remote resource exceeds the {limit}-byte limit")]
    TooLarge {
        /// Maximum response size in bytes.
        limit: usize,
    },
    /// The download worker ended without a response.
    #[error("HTTP task stopped before returning its response")]
    WorkerStopped,
    /// The build attempt was cancelled.
    #[error("{0}")]
    Cancelled(#[from] BuildCancelled),
}

struct HttpDownloads {
    client: reqwest::Client,
    runtime: DownloadRuntime,
}

impl HttpDownloads {
    fn new() -> Result<Self, HttpDownloadError> {
        let runtime = DownloadRuntime(Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(HttpDownloadError::Runtime)?,
        ));
        let client = {
            let _entered = runtime.get().enter();
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(request_error)?
        };
        Ok(Self { client, runtime })
    }

    fn download(
        &self,
        url: &str,
        limit: usize,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<u8>, HttpDownloadError> {
        cancellation.ensure_active()?;
        let url = url.to_owned();
        let client = self.client.clone();
        let worker_cancellation = cancellation.clone();
        let (completed, completion) = mpsc::sync_channel(1);
        let _task = DownloadTask(self.runtime.get().spawn(async move {
            let response = tokio::select! {
                biased;
                cancelled = wait_cancelled(&worker_cancellation) => {
                    Err(HttpDownloadError::Cancelled(cancelled))
                }
                bytes = download_bytes(&client, &url, limit) => bytes,
            };
            let _ = completed.send(response);
        }));
        let response = completion
            .recv()
            .map_err(|_| HttpDownloadError::WorkerStopped)?;
        cancellation.ensure_active()?;
        response
    }
}

struct DownloadTask(tokio::task::JoinHandle<()>);

impl Drop for DownloadTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Uses nonblocking shutdown because it may be dropped on an async thread.
struct DownloadRuntime(Option<tokio::runtime::Runtime>);

impl DownloadRuntime {
    fn get(&self) -> &tokio::runtime::Runtime {
        self.0
            .as_ref()
            .expect("HTTP runtime exists until its owner is dropped")
    }
}

impl Drop for DownloadRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

async fn wait_cancelled(cancellation: &BuildCancellation) -> BuildCancelled {
    loop {
        if let Err(cancelled) = cancellation.ensure_active() {
            return cancelled;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, HttpDownloadError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(request_error)?
        .error_for_status()
        .map_err(request_error)?;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(HttpDownloadError::TooLarge { limit });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_error)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(HttpDownloadError::TooLarge { limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn request_error(error: reqwest::Error) -> HttpDownloadError {
    HttpDownloadError::Request(error.without_url())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view `tola editor setup` publishes is read while every other generated path stays
    /// refused, so a file-only client can open the file a definition named.
    #[test]
    fn package_view_counts_as_source() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let boundary = base_source_boundary(root, InputScope::Online);
        let view = root.join(".tola/builtin-packages/tola/source/0.0.0/lib.typ");
        std::fs::create_dir_all(view.parent().unwrap()).unwrap();
        std::fs::write(&view, "#let value = 1\n").unwrap();
        let sibling = root.join(".tola/inspection/state.json");
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, "{}").unwrap();

        assert!(boundary.check(&view).is_ok(), "{:?}", boundary.check(&view));
        assert_eq!(
            boundary.refusal(&sibling).unwrap(),
            Some(tola_typst::SourceRefusal::GeneratedState)
        );
    }

    /// A site whose output already holds the published `404.html` page.
    fn site_with_published_page(
        directory: &tempfile::TempDir,
    ) -> (crate::config::ResolvedSiteConfig, PathBuf) {
        let config = crate::config::tests::load_test_config(directory.path(), "");
        std::fs::create_dir_all(&config.build.publish_dir).unwrap();
        let published = config.build.publish_dir.join("404.html");
        std::fs::write(&published, "published page").unwrap();
        (config, published)
    }

    #[test]
    fn read_inside_build_output_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let (config, published) = site_with_published_page(&directory);

        let boundary = source_boundary(&config, InputScope::Online);

        assert_eq!(
            boundary.refusal(&published).unwrap(),
            Some(tola_typst::SourceRefusal::GeneratedState)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_build_output_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let (config, published) = site_with_published_page(&directory);
        let alias = config.get_root().join("error.html");
        std::os::unix::fs::symlink(&published, &alias).unwrap();

        let boundary = source_boundary(&config, InputScope::Online);

        assert_eq!(
            boundary.refusal(&alias).unwrap(),
            Some(tola_typst::SourceRefusal::GeneratedState)
        );
    }

    #[test]
    fn sibling_sources_stay_readable() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        std::fs::create_dir_all(&config.build.content_dir).unwrap();
        std::fs::write(&config.build.entry, "#let page = 1").unwrap();
        std::fs::write(config.build.content_dir.join("page.typ"), "#let page = 1").unwrap();

        let boundary = source_boundary(&config, InputScope::Online);

        assert!(boundary.check(&config.build.entry).is_ok());
        assert!(
            boundary
                .check(&config.build.content_dir.join("page.typ"))
                .is_ok()
        );
    }

    #[test]
    fn check_refuses_read_inside_build_output() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        std::fs::create_dir_all(&config.build.content_dir).unwrap();
        std::fs::create_dir_all(&config.build.publish_dir).unwrap();
        std::fs::write(
            config.build.publish_dir.join("value.txt"),
            "written by an earlier publication",
        )
        .unwrap();
        std::fs::write(
            &config.build.entry,
            "#let value = read(\"public/value.txt\")\n#document(\"index.html\")[#value]",
        )
        .unwrap();

        let mut session = crate::check::SourceDiagnosticSession::new(std::sync::Arc::new(config));
        let diagnostics = session
            .check(Vec::new(), &crate::cancellation::BuildCancellation::new())
            .unwrap();

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.severity == crate::diagnostic::Severity::Error
                    && diagnostic.message.contains("public/value.txt")
            }),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn restrictive_scopes_deny_downloads() {
        for resources in [
            BuildResources::new().with_network_access(NetworkAccess::Denied),
            BuildResources::new()
                .with_input_scope(InputScope::Offline)
                .with_network_access(NetworkAccess::Allowed),
            BuildResources::new()
                .with_input_scope(InputScope::Pure)
                .with_network_access(NetworkAccess::Allowed),
            BuildResources::new()
                .with_input_scope(InputScope::Pure)
                .with_input_scope(InputScope::Online)
                .with_network_access(NetworkAccess::Allowed),
        ] {
            let error = resources
                .download(
                    "http://127.0.0.1:1/collection.json",
                    1024,
                    &BuildCancellation::new(),
                )
                .unwrap_err();
            assert!(matches!(error, HttpDownloadError::NetworkDenied));
        }
    }

    #[test]
    fn resources_drop_inside_async_host() {
        let host = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        host.block_on(async {
            let resources = BuildResources::new();
            let downloads = resources.http_downloads().unwrap();
            drop(downloads);
            drop(resources);
        });
    }
}
