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
    #[error("Tola could not fetch the module revision")]
    Other {
        #[source]
        source: gix::Error,
    },
}

/// Fetch `request.revision` of `request.git` into `request.git_dir`.
///
/// The caller creates the destination and inspects cancellation; this function only hands the
/// interrupt flag to gix and reports failures.
pub(crate) fn fetch(
    request: FetchRequest<'_>,
    cancellation: &BundleCancellation,
) -> Result<FetchedRepository, FetchError> {
    let address = || FetchError::Address {
        address: request.git.to_owned(),
    };
    let url = gix::Url::try_from(request.git).map_err(|_| address())?;
    if !matches!(url.scheme, gix::url::Scheme::Https | gix::url::Scheme::Ssh) {
        return Err(address());
    }
    let revision = request.revision.map(|revision| match revision {
        RemoteRevision::Tag(tag) => format!("refs/tags/{tag}"),
        RemoteRevision::Branch(branch) => format!("refs/heads/{branch}"),
        RemoteRevision::Rev(commit) => commit.clone(),
    });
    let options = request.options;
    let mut prepared = gix::clone::PrepareFetch::new(
        url,
        request.git_dir,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        open_options(),
    )
    .map_err(classify)?
    .with_revision(revision)
    .map_err(classify)?
    .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::MIN))
    .configure_connection(move |connection| configure_connection(connection, options));
    let (repository, _) = prepared
        .fetch_only(gix::progress::Discard, cancellation.flag())
        .map_err(classify)?;
    let (commit, tree) = {
        let head = repository.head_commit().map_err(classify)?;
        let tree = head.tree_id().map_err(classify)?.detach();
        (head.id().to_string(), tree)
    };
    Ok(FetchedRepository {
        repository,
        commit,
        tree,
    })
}

/// Open repositories with the user's Git configuration, but with checkout bytes pinned.
///
/// User configuration supplies credential helpers, URL rewrites, proxies, and CAs; environment
/// permissions allow HOME/XDG and the GIT_*/SSH_*/HTTP_* categories those need. Line endings stay
/// fixed so one commit digests identically on every platform. Configured filter drivers are
/// cleared at checkout, so a module's `.gitattributes` cannot run a local filter command.
fn open_options() -> gix::open::Options {
    let mut options = gix::open::Options::isolated();
    options.permissions.config = gix::open::permissions::Config::all();
    // macOS Apple Git keeps `credential.helper = osxkeychain` only in its installation
    // configuration; without this, a standard private HTTPS checkout cannot authenticate.
    options.permissions.config.git_binary = true;
    // The isolated environment permission also hides HOME, which would skip
    // `~/.gitconfig` entirely. Allow the sources a Git user configures: HOME and
    // XDG_CONFIG_HOME for user config, GIT_*/SSH_* for askpass and config overrides,
    // and HTTP_* for proxies. Identity/object environment variables stay isolated.
    let mut env = gix::open::permissions::Environment::isolated();
    env.home = gix::sec::Permission::Allow;
    env.xdg_config_home = gix::sec::Permission::Allow;
    env.git_prefix = gix::sec::Permission::Allow;
    env.ssh_prefix = gix::sec::Permission::Allow;
    env.http_transport = gix::sec::Permission::Allow;
    options.permissions.env = env;
    // `tree_sha256` hashes the materialized bytes, not the Git tree. Pin EOL so the
    // same commit yields the same digest on every host; this is unrelated to auth.
    options.config_overrides(["core.autocrlf=false", "core.eol=lf"])
}

/// Configure the transport and use the user's Git credential helpers.
fn configure_connection(
    connection: &mut gix::remote::Connection<'_, '_, '_, Box<dyn Transport + Send>>,
    options: FetchOptions,
) -> gix::Result<()> {
    connection.set_transport_options(Box::new(http::Options {
        backend: Some(Arc::new(Mutex::new(http::reqwest::Options {
            configure_client: Some(Box::new(move |client| {
                Ok(client
                    .connect_timeout(options.connect_timeout)
                    .timeout(options.request_timeout))
            })),
            configure_request: None,
        }))),
        ..Default::default()
    }));
    // Use gix's built-in cascade: it re-reads helpers per action URL (so redirects
    // follow the user's Git config) and prompts only when no helper supplies credentials.
    // Do not replace it with a prompt-disabled custom cascade.
    let credentials = connection.configured_credentials_for_current_url();
    connection.set_credentials(credentials);
    Ok(())
}

/// Turn one gix failure into the remedy its author can act on.
fn classify(error: gix::Error) -> FetchError {
    if error.is_unauthenticated() || error.is_permission_denied() {
        return FetchError::Authentication { source: error };
    }
    // gix 0.90 renders an unclassified HTTP client status as "Received HTTP status 404 ...".
    // Its version is pinned, so the rendered text is the only stable 404 signal.
    if error
        .iter_errors()
        .any(|cause| cause.to_string().contains("Received HTTP status 404"))
    {
        return FetchError::NotVisible { source: error };
    }
    if error.is_retryable() {
        return FetchError::Network { source: error };
    }
    if io_kinds(&error).any(is_network_kind) {
        return FetchError::Network { source: error };
    }
    if io_kinds(&error).any(|kind| kind == ErrorKind::NotFound) {
        return FetchError::MissingSsh { source: error };
    }
    FetchError::Other { source: error }
}

/// The I/O kinds carried anywhere in an error chain.
fn io_kinds(error: &gix::Error) -> impl Iterator<Item = ErrorKind> + '_ {
    error
        .iter_errors()
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
