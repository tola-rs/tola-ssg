//! The selection index: what every source of one site selects from every other's interface.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::FileId;
use tola_typst_syntax::names::{SelectedInterfaces, SourceNames};

use super::disk::DiskSources;
use super::graph::{add_walked_sources, reach_imports};
use crate::sources::SourceView;

/// What every source of one site selects from every other's interface.
///
/// The index answers whether a name another source may select stays unread, so it holds exactly
/// the sources the name lane holds — the same walk, the same import closure — or the two answer
/// one edit differently: a rename that reaches a source's binding is the other answer to "who else
/// reads this name". The walk therefore skips a dot-named entry as the name lane does, and reads
/// the names the author's ignore files list, because the build reads an ignored `.typ` whenever an
/// import reaches it. A file the walk cannot see that a site path names — a dot-named file, a file
/// inside a hidden directory — joins the set through [`reach_imports`] once an import reaches it.
/// A `@`-package path names a package's own source, which is never a source of the site that
/// spells it, so the closure leaves it out.
///
/// One site's sources are indexed in one order, so the same revision's set answers the same index
/// however the walk itself reached the files, and a caller that stops the check stops this walk.
pub(crate) fn site_selected_interfaces(
    root: &Path,
    view: &SourceView,
    disk: &mut DiskSources,
    cancellation: &BuildCancellation,
) -> Result<Arc<SelectedInterfaces>> {
    let root: &Path = &tola_build::filesystem::normalize_existing_prefix(root);
    let boundary = crate::sources::base_source_boundary(root, false);
    let mut sources: HashMap<FileId, Arc<SourceNames>> = view
        .iter()
        .map(|snapshot| (snapshot.source.id(), snapshot.names()))
        .collect();
    disk.begin();
    add_walked_sources(&mut sources, root, &boundary, cancellation, |path, id| {
        disk.names(path, id)
    })?;
    disk.retain_pass();
    // The index skips a package path itself, so the closure resolves path imports alone: nothing
    // here can read a package the site would have to fetch.
    let mut targets = HashMap::new();
    reach_imports(
        &mut sources,
        &mut targets,
        view,
        root,
        &boundary,
        None,
        &[],
        false,
        cancellation,
    )?;
    let mut sources: Vec<Arc<SourceNames>> = sources.into_values().collect();
    sources.sort_by_key(|names| names.source().id().vpath().get_without_slash().to_owned());
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
