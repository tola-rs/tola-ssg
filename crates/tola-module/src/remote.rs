//! Fetch one remote module revision and leave its tree for checkout.

use std::io::ErrorKind;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gix::protocol::transport::client::blocking_io::{Transport, http};
use tola_typst::BundleCancellation;

use crate::declaration::RemoteRevision;

/// A fetched revision: its bare repository, commit, and tree to materialize.
pub(crate) struct FetchedRepository {
    pub(crate) repository: gix::Repository,
    pub(crate) commit: String,
    pub(crate) tree: gix::ObjectId,
}

/// How long one fetch may spend connecting and waiting for a response.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FetchOptions {
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Duration,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// One module revision to fetch into an empty bare repository.
pub(crate) struct FetchRequest<'a> {
    pub(crate) git: &'a str,
    pub(crate) revision: Option<&'a RemoteRevision>,
    pub(crate) git_dir: &'a Path,
    pub(crate) options: FetchOptions,
}

/// Why the declared revision of a remote module could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("`{address}` is not an `https` or `ssh` address")]
    Address { address: String },
    #[error(
        "the repository needs credentials; configure a Git credential helper for `https`, an SSH key for `ssh`, or use a local `path` module"
    )]
    Authentication {
        #[source]
        source: gix::Error,
    },
    #[error(
        "the repository is not visible; check the address, or configure credentials if it is private"
    )]
    NotVisible {
        #[source]
        source: gix::Error,
    },
    #[error("Tola could not reach the repository; check the network or proxy configuration")]
    Network {
        #[source]
        source: gix::Error,
    },
    #[error("an `ssh` module needs the system `ssh`; install OpenSSH or use an `https` address")]
    MissingSsh {
        #[source]
        source: gix::Error,
    },
    #[error("the remote did not provide the requested revision")]
    RevisionUnavailable {
        #[source]
        source: gix::Error,
    },
    #[error("Tola could not fetch the module revision")]
    Other {
        #[source]
        source: gix::Error,
    },
}

/// Fetch `req.revision` of `req.git` into `req.git_dir`.
///
/// The caller creates the destination and inspects cancellation; this only hands the
/// interrupt flag to gix and classifies failures.
pub(crate) fn fetch(
    req: FetchRequest<'_>,
    cancel: &BundleCancellation,
) -> Result<FetchedRepository, FetchError> {
    let bad_addr = || FetchError::Address {
        address: req.git.to_owned(),
    };
    let url = gix::Url::try_from(req.git).map_err(|_| bad_addr())?;
    if !matches!(url.scheme, gix::url::Scheme::Https | gix::url::Scheme::Ssh) {
        return Err(bad_addr());
    }
    let ssh = url.scheme == gix::url::Scheme::Ssh;
    let map_err = |err| classify(err, ssh, req.revision.is_some());
    let rev = req.revision.map(|rev| match rev {
        RemoteRevision::Tag(tag) => format!("refs/tags/{tag}"),
        RemoteRevision::Branch(branch) => format!("refs/heads/{branch}"),
        RemoteRevision::Rev(commit) => commit.clone(),
    });
    let opts = req.options;
    let mut prepared = gix::clone::PrepareFetch::new(
        url,
        req.git_dir,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        open_options(),
    )
    .map_err(map_err)?
    .with_revision(rev)
    .map_err(map_err)?
    .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::MIN))
    .configure_connection(move |conn| configure_connection(conn, opts));
    let (repo, _) = prepared
        .fetch_only(gix::progress::Discard, cancel.flag())
        .map_err(map_err)?;
    let (commit, tree) = {
        let head = repo.head_commit().map_err(map_err)?;
        let tree = head.tree_id().map_err(map_err)?.detach();
        (head.id().to_string(), tree)
    };
    Ok(FetchedRepository {
        repository: repo,
        commit,
        tree,
    })
}

/// Open repositories with the user's Git config and pinned LF checkout bytes.
fn open_options() -> gix::open::Options {
    let mut opts = gix::open::Options::isolated();
    opts.permissions.config = gix::open::permissions::Config::all();
    // macOS Apple Git keeps `credential.helper = osxkeychain` in its installation config.
    opts.permissions.config.git_binary = true;
    // Allow HOME/XDG/GIT_*/SSH_*/HTTP_* for user config, askpass, and proxies while keeping
    // identity and object environment variables isolated.
    let mut env = gix::open::permissions::Environment::isolated();
    env.home = gix::sec::Permission::Allow;
    env.xdg_config_home = gix::sec::Permission::Allow;
    env.git_prefix = gix::sec::Permission::Allow;
    env.ssh_prefix = gix::sec::Permission::Allow;
    env.http_transport = gix::sec::Permission::Allow;
    opts.permissions.env = env;
    // `tree_sha256` hashes materialized bytes; pin EOL so commits digest identically across hosts.
    opts.config_overrides(["core.autocrlf=false", "core.eol=lf"])
}

fn configure_connection(
    conn: &mut gix::remote::Connection<'_, '_, '_, Box<dyn Transport + Send>>,
    opts: FetchOptions,
) -> gix::Result<()> {
    conn.set_transport_options(Box::new(http::Options {
        backend: Some(Arc::new(Mutex::new(http::reqwest::Options {
            configure_client: Some(Box::new(move |client| {
                Ok(client
                    .connect_timeout(opts.connect_timeout)
                    .timeout(opts.request_timeout))
            })),
            configure_request: None,
        }))),
        ..Default::default()
    }));
    // Use gix's built-in cascade so redirects re-read helpers per URL and prompt only as fallback.
    let creds = conn.configured_credentials_for_current_url();
    conn.set_credentials(creds);
    Ok(())
}

fn classify(err: gix::Error, ssh: bool, explicit_rev: bool) -> FetchError {
    if err.is_unauthenticated() || err.is_permission_denied() {
        return FetchError::Authentication { source: err };
    }
    // gix 0.90 renders an unclassified HTTP client status as "Received HTTP status 404 ...".
    if err
        .iter_errors()
        .any(|cause| cause.to_string().contains("Received HTTP status 404"))
    {
        return FetchError::NotVisible { source: err };
    }
    if err.is_retryable() || io_kinds(&err).any(is_network_kind) {
        return FetchError::Network { source: err };
    }
    let missing_file = io_kinds(&err).any(|kind| kind == ErrorKind::NotFound);
    if ssh && missing_file {
        return FetchError::MissingSsh { source: err };
    }
    if explicit_rev && err.is_not_found() && !missing_file {
        return FetchError::RevisionUnavailable { source: err };
    }
    FetchError::Other { source: err }
}

fn io_kinds(err: &gix::Error) -> impl Iterator<Item = ErrorKind> + '_ {
    err.iter_errors()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind)
}

/// I/O kinds that describe an unreachable peer rather than a local problem.
fn is_network_kind(kind: ErrorKind) -> bool {
    use ErrorKind::*;
    matches!(
        kind,
        ConnectionRefused
            | ConnectionReset
            | ConnectionAborted
            | NotConnected
            | AddrNotAvailable
            | BrokenPipe
            | UnexpectedEof
            | TimedOut
    )
}
