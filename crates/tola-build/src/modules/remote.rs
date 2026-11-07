use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use gix::protocol::transport::client::blocking_io::http;

use super::RemoteRevision;
use crate::cancellation::BuildCancellation;

pub(crate) struct FetchedRevision {
    pub(crate) commit: String,
    pub(crate) root: PathBuf,
}

pub(crate) fn fetch(
    git: &str,
    revision: Option<&RemoteRevision>,
    destination: &Path,
    cancellation: &BuildCancellation,
) -> Result<FetchedRevision> {
    cancellation.ensure_active()?;
    let url = gix::Url::try_from(git)?;
    ensure!(
        url.scheme == gix::url::Scheme::Https,
        "a module `git` must be an `https` URL"
    );
    let revision: Option<String> = revision.map(|revision| match revision {
        RemoteRevision::Tag(tag) => format!("refs/tags/{tag}"),
        RemoteRevision::Branch(branch) => format!("refs/heads/{branch}"),
        RemoteRevision::Rev(commit) => commit.clone(),
    });
    std::fs::create_dir_all(destination)?;
    // Isolated configuration excludes Git discovery, credential helpers, and configured filter commands.
    let mut prepared = gix::clone::PrepareFetch::new(
        url,
        destination.join("objects.git"),
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )?
    .with_revision(revision)?
    .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::MIN))
    .configure_connection(|connection| {
        connection.set_credentials(|_| Ok(None));
        connection.set_transport_options(Box::new(http::Options {
            backend: Some(Arc::new(Mutex::new(http::reqwest::Options {
                configure_client: Some(Box::new(|client| {
                    Ok(client
                        .connect_timeout(Duration::from_secs(10))
                        .timeout(Duration::from_secs(30)))
                })),
                configure_request: None,
            }))),
            ..Default::default()
        }));
        Ok(())
    });
    let fetched = prepared.fetch_only(gix::progress::Discard, cancellation.flag());
    cancellation.ensure_active()?;
    let (repository, _) = fetched.context("Tola could not fetch the module from its repository")?;
    let commit = repository.head_commit()?;
    let mut index = repository.index_from_tree(&commit.tree_id()?)?;
    let mut options =
        repository.checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)?;
    options.destination_is_initially_empty = true;
    let root = destination.join("source");
    std::fs::create_dir(&root)?;
    cancellation.ensure_active()?;
    let checkout = gix::worktree::state::checkout(
        &mut index,
        &root,
        repository.objects.clone().into_arc()?,
        &gix::progress::Discard,
        &gix::progress::Discard,
        cancellation.flag(),
        options,
    );
    // gix can return a successful partial checkout after interruption.
    cancellation.ensure_active()?;
    let checkout = checkout.context("Tola could not extract the fetched module sources")?;
    ensure!(
        checkout.collisions.is_empty() && checkout.errors.is_empty(),
        "the fetched module holds files Tola could not extract"
    );
    Ok(FetchedRevision {
        commit: commit.id().to_string(),
        root,
    })
}
