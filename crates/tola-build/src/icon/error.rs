use std::path::{Path, PathBuf};

use tola_diagnostic::{Diagnostic, Severity};

/// A remote icon collection did not match its recorded digest or size.
#[derive(Debug, thiserror::Error)]
#[error(
    "SHA-256 mismatch for icon collection `{namespace}` from `{url}`: expected {expected}, received {received}"
)]
pub struct IconIntegrityError {
    pub(crate) namespace: String,
    pub(crate) url: String,
    pub(crate) expected: String,
    pub(crate) received: String,
    pub(crate) size_expected: u64,
    pub(crate) size_received: u64,
    pub(crate) path: Option<PathBuf>,
}

/// An icon collection could not be read, checked, or fetched.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IconError {
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
    #[error(transparent)]
    Integrity(Box<IconIntegrityError>),
    #[error("Tola may not read icon source `{path}`")]
    Boundary {
        path: PathBuf,
        #[source]
        source: typst::diag::FileError,
    },
    #[error("the npm registry returned no usable metadata for preset `{preset}`")]
    RegistryFormat { preset: String },
    #[error("preset `{preset}` has no release `{version}`")]
    RegistryRelease {
        preset: String,
        version: String,
        latest: Option<String>,
    },
    #[error("preset `{preset}` release `{version}` publishes no integrity digest")]
    RegistryIntegrity { preset: String, version: String },
    #[error("Tola could not read the npm registry metadata for preset `{preset}`")]
    RegistryMetadata {
        namespace: String,
        preset: String,
        url: String,
        #[source]
        source: crate::HttpDownloadError,
    },
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
    #[error("Tola could not read the tarball of preset `{preset}` release `{version}`")]
    Tarball {
        namespace: String,
        preset: String,
        version: String,
    },
    #[error(
        "the tarball of preset `{preset}` release `{version}` exceeds the {limit}-byte collection limit"
    )]
    TarballCollectionTooLarge {
        namespace: String,
        preset: String,
        version: String,
        limit: usize,
    },
    #[error("the tarball of preset `{preset}` release `{version}` holds no collection file")]
    TarballCollection {
        namespace: String,
        preset: String,
        version: String,
    },
    #[error("icon collection `{namespace}` is not locked; run `tola update {namespace}`")]
    Unlocked { namespace: String },
    #[error(
        "icon collection `{namespace}` no longer matches `tola.lock`; run `tola update {namespace}`"
    )]
    LockMismatch {
        namespace: String,
        recorded: String,
        declared: String,
    },
    #[error("icon collection `{namespace}` has no bytes for digest {sha256}; run `tola vendor`")]
    MissingBytes {
        namespace: String,
        sha256: tola_input::Sha256Digest,
    },
    #[error("icon collection `{namespace}` names neither a preset nor a URL")]
    NotRemote { namespace: String },
    #[error("refreshing icon collections writes `tola.lock`; run without `--locked` or `--pure`")]
    RefreshLocked,
    #[error(transparent)]
    Lock(#[from] tola_input::LockError),
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
            Self::Integrity(err) => err.path.as_deref(),
            _ => None,
        }
    }
}

pub(super) fn collection_error(
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
        .map(|h| format!("`{h}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collection_failure_reason(source: &(dyn std::error::Error + Send + Sync)) -> Option<String> {
    source
        .to_string()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

pub(crate) fn error_diagnostic(error: &anyhow::Error, root: &Path) -> Option<Diagnostic> {
    let err = error.chain().find_map(|s| s.downcast_ref::<IconError>())?;
    let diag = Diagnostic::new(
        tola_diagnostic::codes::icons::COLLECTION,
        Severity::Error,
        err.to_string(),
    );
    let diag = match err {
        IconError::Read { source, .. } => diag
            .with_note(crate::filesystem::path_failure_reason(source))
            .with_help("Check that the icon source exists and that Tola can read it"),
        IconError::Changed { .. } => {
            diag.with_help("Run the build again without changing icon sources")
        }
        IconError::TooLarge { .. } => {
            diag.with_help("Split the icons across several `icons.collections` entries")
        }
        IconError::InvalidName { .. } => {
            diag.with_note("icon names drop the `.svg` extension and write `/` as `-`")
        }
        IconError::LinkedDirectoryMember { .. } => diag.with_help(
            "Replace the link with the SVG file it points at, or declare that file as its own `icons.collections` entry",
        ),
        IconError::InternalSource { .. } => {
            diag.with_help("Move the icon source outside `.tola` and update `icons.collections`")
        }
        IconError::Collection { source, .. } => match collection_failure_reason(source.as_ref()) {
            Some(reason) => diag.with_note(reason),
            None => diag,
        },
        IconError::Remote { urls, .. } => {
            let diag = match urls.len() {
                2.. => diag.with_note(format!("Tola tried {}", tried_hosts(urls))),
                _ => diag,
            };
            diag.with_help("Check the connection and the address")
        }
        IconError::RemoteTooLarge { .. } => diag.with_help(
            "Use a smaller collection, or declare the icons as a local file in `icons.collections`",
        ),
        IconError::NetworkDenied { .. } => {
            diag.with_help("Run `tola vendor` while online, or declare a local icon collection")
        }
        IconError::Integrity(_) => diag
            .with_note("run `tola update` only when you intend to accept changed collection bytes")
            .with_help("Run `tola vendor --refresh` to restore the site's copy"),
        IconError::NetworkRuntime(_) => {
            diag.with_help("Check the connection, then run the build again")
        }
        IconError::NetworkWorker => diag.with_help("Run the build again"),
        _ => diag,
    };
    Some(match err.path() {
        Some(path) => diag.with_path(tola_diagnostic::display_path(path, root)),
        None => diag,
    })
}
