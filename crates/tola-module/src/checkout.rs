//! Materialize a fetched tree into the module directory.

use std::path::Path;

use tola_typst::BundleCancellation;

/// Why the fetched tree could not become the module directory.
#[derive(Debug, thiserror::Error)]
pub enum CheckoutError {
    #[error("Tola could not prepare the module directory")]
    Directory {
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not read the fetched revision")]
    Revision {
        #[source]
        source: gix::Error,
    },
    #[error("Tola could not prepare the module checkout")]
    Filters {
        #[source]
        source: gix::Error,
    },
    #[error("Tola could not extract the fetched module sources")]
    Extract {
        #[source]
        source: gix::Error,
    },
    #[error("the fetched module holds files Tola could not extract")]
    Collision,
}

/// Write `tree` from `repository` into the empty directory `root`.
pub(crate) fn materialize(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    root: &Path,
    cancel: &BundleCancellation,
) -> Result<(), CheckoutError> {
    let mut index = repo
        .index_from_tree(&tree)
        .map_err(|source| CheckoutError::Revision { source })?;
    let mut opts = repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .map_err(|source| CheckoutError::Revision { source })?;
    opts.destination_is_initially_empty = true;
    // Clear `.gitattributes` filter drivers so module content never invokes local commands;
    // keep EOL conversion pinned for deterministic bytes.
    let mut filters =
        gix::filter::Pipeline::options(repo).map_err(|source| CheckoutError::Filters { source })?;
    filters.drivers.clear();
    opts.filters = gix::filter::plumbing::Pipeline::new(
        repo.command_context()
            .map_err(|source| CheckoutError::Filters { source })?,
        repo.object_hash(),
        filters,
    );
    std::fs::create_dir(root).map_err(|source| CheckoutError::Directory { source })?;
    let objs = repo
        .objects
        .clone()
        .into_arc()
        .map_err(|source| CheckoutError::Revision {
            source: gix::Error::from_error(source),
        })?;
    let out = gix::worktree::state::checkout(
        &mut index,
        root,
        objs,
        &gix::progress::Discard,
        &gix::progress::Discard,
        cancel.flag(),
        opts,
    )
    // gix yields a successful partial checkout after interruption; caller rechecks cancellation.
    .map_err(|source| CheckoutError::Extract { source })?;
    if !out.collisions.is_empty() || !out.errors.is_empty() {
        return Err(CheckoutError::Collision);
    }
    Ok(())
}
