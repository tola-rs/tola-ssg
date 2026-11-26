//! Filesystem changes for development rebuilds.

use std::path::PathBuf;
use std::sync::Arc;

mod boundary;
mod debouncer;
mod dispatch;
mod epoch;
mod event;
mod observer;
mod path;
mod producer;
mod queue;
mod roots;
mod source;

#[cfg(test)]
mod tests;

pub(crate) use boundary::WatchSelection;
use dispatch::generated_source_identities;
pub(crate) use epoch::EventClaim;
pub(crate) use producer::{ProducerKind, WatchRequirements};
pub(crate) use source::FileChangeSource;

/// Shared bound for retained event-detail containers; spill stays owed by its epoch.
pub(crate) const EVENT_DETAIL_CAPACITY: usize = 256;

/// Failure to start or maintain filesystem watching.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WatchError {
    #[error("failed to create filesystem watcher: {0}")]
    Create(#[source] notify::Error),
    #[error("filesystem watcher reported an error: {0}")]
    Event(#[source] Arc<notify::Error>),
    #[error("filesystem watcher stopped unexpectedly")]
    Stopped,
    #[error("failed to observe generated inputs: {0}")]
    HookInputs(#[source] anyhow::Error),
    #[error("could not watch `rerun-on` paths: {0}")]
    RerunOn(#[source] anyhow::Error),
    #[error("could not watch a configured path")]
    Attach {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

/// Rebuild scope required by accepted filesystem changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum RebuildScope {
    /// Rebuild only producers affected by the accepted paths.
    #[default]
    Paths,
    /// Recompile every Typst unit while retaining non-Typst inventories.
    FullTypst,
    /// Rebuild every producer from a fresh site observation.
    FullSite,
}

/// A debounced batch of accepted filesystem changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileChangeBatch {
    paths: Vec<PathBuf>,
    rebuild_scope: RebuildScope,
}

impl FileChangeBatch {
    pub(crate) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub(crate) fn into_parts(self) -> (Vec<PathBuf>, RebuildScope) {
        (self.paths, self.rebuild_scope)
    }
}
