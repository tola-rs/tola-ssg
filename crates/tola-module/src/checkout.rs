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
    repository: &gix::Repository,
    tree: gix::ObjectId,
    root: &Path,
    cancellation: &BundleCancellation,
) -> Result<(), CheckoutError> {
    let mut index = repository
        .index_from_tree(&tree)
        .map_err(|source| CheckoutError::Revision { source })?;
    let mut options = repository
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .map_err(|source| CheckoutError::Revision { source })?;
    options.destination_is_initially_empty = true;
    // A repository's `.gitattributes` can select a filter driver from the user's
    // configuration, and `permissions.attributes` does not stop that program. Clear the
    // drivers so module content never invokes a local command; keep EOL conversion,
    // which is pinned for deterministic bytes.
    let mut filters = gix::filter::Pipeline::options(repository)
        .map_err(|source| CheckoutError::Filters { source })?;
    filters.drivers.clear();
    options.filters = gix::filter::plumbing::Pipeline::new(
        repository
            .command_context()
            .map_err(|source| CheckoutError::Filters { source })?,
        repository.object_hash(),
        filters,
    );
    std::fs::create_dir(root).map_err(|source| CheckoutError::Directory { source })?;
    let objects =
        repository
            .objects
            .clone()
            .into_arc()
            .map_err(|source| CheckoutError::Revision {
                source: gix::Error::from_error(source),
            })?;
    let checkout = gix::worktree::state::checkout(
        &mut index,
        root,
        objects,
        &gix::progress::Discard,
        &gix::progress::Discard,
        cancellation.flag(),
        options,
    );
    // gix yields a successful partial checkout after interruption; the caller rechecks
    // cancellation before trusting the directory.
    let checkout = checkout.map_err(|source| CheckoutError::Extract { source })?;
    if !checkout.collisions.is_empty() || !checkout.errors.is_empty() {
        return Err(CheckoutError::Collision);
    }
    Ok(())
}
