//! The selection index: what every source of one site selects from every other's interface.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_typst::SourceBoundary;
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::FileId;
use tola_typst_syntax::names::{SelectedInterfaces, SourceNames};

use super::by_source_path;
use super::disk::DiskSources;
use super::graph::{
    PackageSources, ReachSources, add_walked_sources, reach_sources, source_within_boundary,
};
use crate::sources::SourceView;

/// What every source of one site selects from every other's interface.
///
/// The index answers whether a name another source may select stays unread, so it holds exactly
/// the sources the name lane holds — the same walk, the same import closure — or the two answer
/// one edit differently: a rename that reaches a source's binding is the other answer to "who else
/// reads this name". The walk therefore skips a dot-named entry as the name lane does, and reads
/// the names the author's ignore files list, because the build reads an ignored `.typ` whenever an
/// import reaches it. A file the walk cannot see that a site path names — a dot-named file, a file
/// inside a hidden directory — joins the set through [`reach_sources`] once an import or include
/// reaches it.
/// A `@`-package path names a package's own source, which is never a source of the site that
/// spells it, so the closure leaves it out.
///
/// One site's sources are indexed in one order, so the same revision's set answers the same index
/// however the walk itself reached the files, and a caller that stops the check stops this walk.
pub(crate) fn site_selected_interfaces(
    root: &Path,
    view: &SourceView,
    boundary: &SourceBoundary,
    disk: &mut DiskSources,
    cancellation: &BuildCancellation,
) -> Result<Arc<SelectedInterfaces>> {
    let sources = site_sources(root, view, boundary, disk, cancellation)?;
    Ok(Arc::new(tola_typst_syntax::names::selected_interfaces(
        sources.iter().map(AsRef::as_ref),
        |file, path| {
            PathOrStr::Str(path.into())
                .resolve(file)
                .ok()
                .map(|resolved| resolved.intern())
        },
    )))
}

/// The site's parsed sources, including open documents and their local imports and includes.
pub(crate) fn site_sources(
    root: &Path,
    view: &SourceView,
    boundary: &SourceBoundary,
    disk: &mut DiskSources,
    cancellation: &BuildCancellation,
) -> Result<Vec<Arc<SourceNames>>> {
    let boundary = view.site_boundary(boundary);
    let boundary = boundary.as_ref();
    let root: &Path = &tola_input::normalize_existing_prefix(root);
    let mut sources: HashMap<FileId, Arc<SourceNames>> = view
        .iter()
        .filter(|parsed| source_within_boundary(parsed.source.id(), root, boundary))
        .map(|parsed| (parsed.source.id(), parsed.names()))
        .collect();
    disk.begin();
    add_walked_sources(&mut sources, root, boundary, disk, cancellation)?;
    // The index skips a package path itself, so the closure resolves path imports alone: nothing
    // here can read a package the site would have to fetch.
    let mut targets = HashMap::new();
    reach_sources(ReachSources {
        sources: &mut sources,
        targets: &mut targets,
        view,
        root,
        boundary,
        resolver: None,
        established: &[],
        package_sources: PackageSources::Skipped,
        disk: &mut *disk,
        cancellation,
    })?;
    disk.retain_pass();
    let mut sources: Vec<Arc<SourceNames>> = sources.into_values().collect();
    sources.sort_by(|left, right| by_source_path(&left.source().id(), &right.source().id()));
    Ok(sources)
}
