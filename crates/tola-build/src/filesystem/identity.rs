//! The watch boundaries and watch evidence one observed source defines.

use std::path::Path;

use tola_input::FilesystemSourceIdentity;

/// Whether one observed source is an exact file or a recursively enumerated tree.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum FilesystemSourceKind {
    Tree,
    File,
}

/// A source's logical and physical paths with its file or recursive-tree watch scope.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FilesystemWatchBoundary {
    kind: FilesystemSourceKind,
    source: FilesystemSourceIdentity,
}

impl FilesystemWatchBoundary {
    pub(crate) fn kind(&self) -> FilesystemSourceKind {
        self.kind
    }

    pub(crate) fn logical_path(&self) -> &Path {
        self.source.logical_path()
    }

    pub(crate) fn physical_path(&self) -> &Path {
        self.source.physical_path()
    }
}

/// Exact file and recursive tree watch requirements from frozen observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FilesystemWatchEvidence {
    boundaries: Vec<FilesystemWatchBoundary>,
}

impl FilesystemWatchEvidence {
    pub(crate) fn from_sources(
        sources: impl IntoIterator<Item = (FilesystemSourceKind, FilesystemSourceIdentity)>,
    ) -> Self {
        let mut boundaries = sources
            .into_iter()
            .map(|(kind, source)| FilesystemWatchBoundary { kind, source })
            .collect::<Vec<_>>();
        boundaries.sort_unstable();
        boundaries.dedup();
        Self { boundaries }
    }

    pub(crate) fn boundaries(&self) -> &[FilesystemWatchBoundary] {
        &self.boundaries
    }
}
