//! Building the name graph over a site's sources.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::package::{PackageManifest, PackageSpec};
use tola_typst::typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use tola_typst::{
    FileResolver, GLOBAL_LIBRARY, PackageFetchPolicy, PackageLocations, SourceBoundary,
};
use tola_typst_syntax::imports::{ImportQuery, NameGraph, PendingImport};
use tola_typst_syntax::names::{ImportSource, SourceNames};

use super::disk::DiskSources;
use crate::sources::SourceView;

/// The graph cache one request may reuse, and the revision its reuse must match.
pub(crate) struct CachedGraph<'a> {
    pub(crate) cache: &'a mut GraphCache,
    pub(crate) revision: u64,
}
/// One source-only graph, reusable after every reached source and import target is revalidated.
/// Directory scans alone omit hidden imports and package sources.
#[derive(Default)]
pub(crate) struct GraphCache {
    revision: u64,
    targets: HashMap<FileId, BTreeMap<String, FileId>>,
    graph: Option<Arc<NameGraph>>,
}

impl GraphCache {
    fn find(
        &self,
        revision: u64,
        sources: &HashMap<FileId, Arc<SourceNames>>,
        targets: &HashMap<FileId, BTreeMap<String, FileId>>,
    ) -> Option<Arc<NameGraph>> {
        let graph = self.graph.as_ref()?;
        (self.revision == revision
            && self.targets == *targets
            && graph.sources().count() == sources.len()
            && sources.iter().all(|(file, names)| {
                graph
                    .source(*file)
                    .is_some_and(|cached| cached.source().text() == names.source().text())
            }))
        .then(|| Arc::clone(graph))
    }

    fn store(
        &mut self,
        revision: u64,
        targets: HashMap<FileId, BTreeMap<String, FileId>>,
        graph: Arc<NameGraph>,
    ) {
        self.revision = revision;
        self.targets = targets;
        self.graph = Some(graph);
    }
}

/// Builds the source-only graph: no import expression has a host-established target.
///
/// The connection's direct answers and the analysis lane build their graphs through this shape;
/// the query lane builds the same graph with the targets the compiler proved through [`graph`].
#[expect(
    clippy::too_many_arguments,
    reason = "the source graph reads root, view, source, resolution inputs, disk, cache, and cancellation"
)]
pub(crate) fn source_only_graph(
    root: &Path,
    view: &SourceView,
    source: &Source,
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
    include_dependents: bool,
    disk: &mut DiskSources,
    cache: Option<CachedGraph<'_>>,
    cancellation: &BuildCancellation,
) -> Result<Arc<NameGraph>> {
    graph(
        root,
        view,
        source,
        locations,
        boundary,
        include_dependents,
        &[],
        disk,
        cache,
        cancellation,
    )
}

/// Builds the graph over one source set, with host-established targets for import expressions.
///
/// `resolved` holds the targets the official compiler semantics proved for statements
/// [`NameGraph::unresolved_imports`] reported; their sources join the graph so the identity
/// those statements provide becomes visible to every name query.
#[expect(
    clippy::too_many_arguments,
    reason = "the graph reads root, view, source, resolution inputs, proven targets, disk, cache, and cancellation"
)]
pub(crate) fn graph(
    root: &Path,
    view: &SourceView,
    source: &Source,
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
    include_dependents: bool,
    resolved: &[(PendingImport, FileId)],
    disk: &mut DiskSources,
    cache: Option<CachedGraph<'_>>,
    cancellation: &BuildCancellation,
) -> Result<Arc<NameGraph>> {
    cancellation.ensure_active()?;
    // Every path this graph reads or compares is a filesystem path, and a client may name the
    // root through a symlink: the resolved spelling is what `VirtualPath::virtualize` accepts,
    // so the dependents walk reaches the site's files under the identity their readers hold.
    let root: &Path = &tola_build::filesystem::normalize_existing_prefix(root);
    if view.is_unnamed(source.id()) {
        let names = view
            .by_id(source.id())
            .map(|snapshot| snapshot.names())
            .unwrap_or_else(|| Arc::new(SourceNames::new(source.clone())));
        return Ok(Arc::new(NameGraph::new(
            [names],
            |_| None,
            Some(&GLOBAL_LIBRARY),
            || cancellation.ensure_active(),
        )?));
    }
    let resolver = file_resolver(locations, boundary);
    let mut sources = HashMap::new();
    let names = view
        .by_id(source.id())
        .map(|snapshot| snapshot.names())
        .unwrap_or_else(|| Arc::new(SourceNames::new(source.clone())));
    sources.insert(source.id(), names);
    if include_dependents {
        // The walk below closes the pass it opens; a cancellation returns through `?` and leaves
        // it open, and the next completed pass drops what this one did not see.
        disk.begin();
        for snapshot in view.iter() {
            if source_allowed(snapshot.source.id(), root, boundary) {
                sources.insert(snapshot.source.id(), snapshot.names());
            }
        }
        // The walk reaches every `.typ` the site holds: the build reads whatever an import
        // reaches, so a file the author's own ignore file lists is still one a consumer's
        // reference must reach.
        add_walked_sources(&mut sources, root, boundary, cancellation, |path, id| {
            disk.names(path, id)
        })?;
        disk.retain_pass();
    }
    let established: Vec<(PendingImport, FileId)> = resolved
        .iter()
        .copied()
        .filter(|(_, target)| source_allowed(*target, root, boundary))
        .collect();
    let mut targets = HashMap::new();
    reach_imports(
        &mut sources,
        &mut targets,
        view,
        root,
        boundary,
        resolver.as_ref(),
        &established,
        true,
        cancellation,
    )?;
    if let Some(cached) = cache.as_ref()
        && resolved.is_empty()
        && let Some(graph) = cached.cache.find(cached.revision, &sources, &targets)
    {
        return Ok(graph);
    }
    // A host-established target belongs to the statement's source expression, the key the graph
    // asks its resolver with.
    let mut statements = HashMap::new();
    for (pending, target) in &established {
        if let Some(names) = sources.get(&pending.file)
            && let Some(import) = names.imports().get(pending.statement)
        {
            statements.insert((pending.file, import.source_range.clone()), *target);
        }
    }
    let graph = Arc::new(NameGraph::new(
        sources.into_values(),
        |query| match query {
            ImportQuery::Path { file, path } => targets.get(&file)?.get(path).copied(),
            ImportQuery::Expression { file, range } => statements.get(&(file, range)).copied(),
        },
        Some(&GLOBAL_LIBRARY),
        || cancellation.ensure_active(),
    )?);
    if let Some(cached) = cache
        && resolved.is_empty()
    {
        cached
            .cache
            .store(cached.revision, targets, Arc::clone(&graph));
    }
    Ok(graph)
}
/// The walk that discovers the site's Typst sources.
///
/// The author's ignore files are not consulted: the build reads an ignored `.typ` whenever an
/// import reaches it, so a file they list is still one the site's own imports and references
/// reach. A dot-named entry is skipped, and Tola's generated state is refused by the boundary, so
/// the walk descends into what a source can be written in; a dot-named file a site path names
/// joins the source set through [`reach_imports`] once an import reaches it.
fn source_walk(root: &Path, boundary: &SourceBoundary) -> ignore::WalkBuilder {
    let boundary = boundary.clone();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .filter_entry(move |entry| {
            !entry.file_type().is_some_and(|kind| kind.is_dir())
                || source_directory(entry, &boundary)
        });
    walker
}

/// Add every `.typ` file the site walk reaches to `sources`, parsed as `read_names` reads it.
///
/// An id already present is left as it stands: an open document or an import target has the
/// text its own reader holds, which takes precedence over the file the walk would read.
pub(super) fn add_walked_sources(
    sources: &mut HashMap<FileId, Arc<SourceNames>>,
    root: &Path,
    boundary: &SourceBoundary,
    cancellation: &BuildCancellation,
    mut read_names: impl FnMut(&Path, FileId) -> Option<Arc<SourceNames>>,
) -> Result<()> {
    for entry in source_walk(root, boundary).build().filter_map(Result::ok) {
        cancellation.ensure_active()?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "typ")
        {
            continue;
        }
        if boundary.check(entry.path()).is_err() {
            continue;
        }
        let Some(id) = crate::identity::path_id(entry.path(), root) else {
            continue;
        };
        if sources.contains_key(&id) {
            continue;
        }
        if let Some(names) = read_names(entry.path(), id) {
            sources.insert(id, names);
        }
    }
    Ok(())
}

/// Read every source the given sources' imports reach, to a fixed point.
///
/// `targets` records the file each path statement resolves to, which is the resolver the name
/// graph answers `ImportQuery::Path` with. A target the given set cannot see — a file inside a
/// hidden directory, an embedded package source — is read from wherever the name layer reads it,
/// and a target nothing answers for is left out: a file no reader produced has no identity.
///
/// `package_targets` decides whether a `@`-package path's own source joins the set. The name
/// graph resolves every import the site spells, while an index of what the site's own sources
/// select from each other holds no package source: a package's sources are never sources of the
/// site that spells it, and their own imports must not decide what a site statement selects.
#[expect(
    clippy::too_many_arguments,
    reason = "the closure reads the source set, targets, view, root, boundary, resolver, proven targets, package policy, and cancellation"
)]
pub(super) fn reach_imports(
    sources: &mut HashMap<FileId, Arc<SourceNames>>,
    targets: &mut HashMap<FileId, BTreeMap<String, FileId>>,
    view: &SourceView,
    root: &Path,
    boundary: &SourceBoundary,
    resolver: Option<&FileResolver>,
    established: &[(PendingImport, FileId)],
    package_targets: bool,
    cancellation: &BuildCancellation,
) -> Result<()> {
    let mut visited = HashSet::new();
    loop {
        cancellation.ensure_active()?;
        let pending: Vec<_> = sources
            .keys()
            .copied()
            .filter(|id| !visited.contains(id))
            .collect();
        if pending.is_empty() {
            break;
        }
        for file in pending {
            cancellation.ensure_active()?;
            visited.insert(file);
            let imports: Vec<String> = sources[&file]
                .imports()
                .iter()
                .filter_map(|import| match &import.source {
                    ImportSource::Path(path) if package_targets || !path.starts_with('@') => {
                        Some(path.clone())
                    }
                    _ => None,
                })
                .collect();
            let mut reached = Vec::new();
            for path in imports {
                cancellation.ensure_active()?;
                let Some(target) = import_target(file, &path, root, resolver) else {
                    continue;
                };
                if !source_allowed(target, root, boundary) {
                    continue;
                }
                targets.entry(file).or_default().insert(path, target);
                reached.push(target);
            }
            reached.extend(
                established
                    .iter()
                    .filter(|(pending, _)| pending.file == file)
                    .map(|(_, target)| *target),
            );
            for target in reached {
                cancellation.ensure_active()?;
                if sources.contains_key(&target) {
                    continue;
                }
                if let Some(snapshot) = view.by_id(target) {
                    sources.insert(target, snapshot.names());
                    continue;
                }
                let text = crate::identity::embedded_source(target)
                    .map(|text| text.into_owned())
                    .or_else(|| {
                        resolver.and_then(|resolver| {
                            String::from_utf8(resolver.read(target, root).ok()?).ok()
                        })
                    })
                    .or_else(|| {
                        matches!(target.root(), VirtualRoot::Project)
                            .then(|| {
                                let path = root.join(target.vpath().get_without_slash());
                                boundary.check(&path).ok()?;
                                std::fs::read_to_string(path).ok()
                            })
                            .flatten()
                    });
                if let Some(text) = text {
                    sources.insert(
                        target,
                        Arc::new(SourceNames::new(Source::new(target, text))),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Whether the walk descends into this directory.
///
/// A directory holds no source when the boundary refuses it — Tola's generated state, the
/// published output, the vendor workspace — or when it is tool state a site root may hold:
/// Cargo's `target` and npm's `node_modules` hold generated files the build never reads. The
/// walk's own hidden filter already skips a dot-named directory, and a dot-named file a site
/// path names joins the source set through [`reach_imports`] when an import reaches it.
fn source_directory(entry: &ignore::DirEntry, boundary: &SourceBoundary) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    if matches!(entry.file_name().to_str(), Some("target" | "node_modules")) {
        return false;
    }
    boundary.check(entry.path()).is_ok()
}

fn source_allowed(id: FileId, root: &Path, boundary: &SourceBoundary) -> bool {
    !matches!(id.root(), VirtualRoot::Project)
        || boundary
            .check(&root.join(id.vpath().get_without_slash()))
            .is_ok()
}

/// The resolver every import path resolves through: packages read locally, sources inside
/// `boundary`.
pub(crate) fn file_resolver(
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
) -> Option<FileResolver> {
    locations.map(|locations| {
        FileResolver::from_package_locations(locations.clone(), PackageFetchPolicy::LocalOnly)
            .with_source_boundary(boundary.clone())
    })
}

/// The source one import path names, resolved exactly as the compiler resolves it.
pub(crate) fn import_target(
    file: FileId,
    path: &str,
    root: &Path,
    resolver: Option<&FileResolver>,
) -> Option<FileId> {
    if !path.starts_with('@') {
        return Some(PathOrStr::Str(path.into()).resolve(file).ok()?.intern());
    }
    let spec: PackageSpec = path.parse().ok()?;
    let manifest_id = RootedPath::new(
        VirtualRoot::Package(spec.clone()),
        VirtualPath::new("typst.toml").ok()?,
    )
    .intern();
    let text = crate::identity::embedded_source(manifest_id)
        .map(|text| text.into_owned())
        .or_else(|| String::from_utf8(resolver?.read(manifest_id, root).ok()?).ok())?;
    let manifest: PackageManifest = toml::from_str(&text).ok()?;
    manifest.validate(&spec).ok()?;
    Some(
        RootedPath::new(
            VirtualRoot::Package(spec),
            VirtualPath::new(&manifest.package.entrypoint).ok()?,
        )
        .intern(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::OpenSources;
    use lsp_types::notification::{DidOpenTextDocument, Notification};
    use tola_build::cancellation::BuildCancellation;

    fn open(sources: &mut OpenSources, root: &Path, path: &str, text: &str) -> lsp_types::Uri {
        let uri = crate::uri::from_file_path(&root.join(path)).unwrap();
        sources
            .apply(lsp_server::Notification::new(
                DidOpenTextDocument::METHOD.into(),
                lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem {
                        uri: uri.clone(),
                        language_id: "typst".into(),
                        version: 1,
                        text: text.into(),
                    },
                },
            ))
            .unwrap();
        uri
    }

    #[test]
    fn cached_definitions_follow_hidden_imports() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let library = root.join(".library.typ");
        let mut sources = OpenSources::new(root);
        let importer = open(
            &mut sources,
            root,
            "main.typ",
            "#import \".library.typ\": marker\n#marker",
        );
        let view = sources.view();
        let source = &view.get(&importer).unwrap().source;
        let cursor = source.text().rfind("marker").unwrap();
        let boundary = tola_typst::SourceBoundary::new(root, true);
        let cancellation = BuildCancellation::new();
        let mut disk = DiskSources::default();
        let mut cache = GraphCache::default();
        for text in ["#let marker = 1", "\n\n#let marker = 2"] {
            std::fs::write(&library, text).unwrap();
            let cached = source_only_graph(
                root,
                &view,
                source,
                None,
                &boundary,
                true,
                &mut disk,
                Some(CachedGraph {
                    cache: &mut cache,
                    revision: sources.revision(),
                }),
                &cancellation,
            )
            .unwrap();
            let fresh = source_only_graph(
                root,
                &view,
                source,
                None,
                &boundary,
                true,
                &mut DiskSources::default(),
                None,
                &cancellation,
            )
            .unwrap();
            let definition = |graph: &NameGraph| {
                graph.definition(
                    graph
                        .selected(source.id(), cursor)
                        .unwrap()
                        .binding
                        .unwrap(),
                )
            };
            assert_eq!(definition(&cached), definition(&fresh));
            let (_, range) = definition(&cached).unwrap();
            assert_eq!(range.start, text.find("marker").unwrap());
        }
    }

    #[test]
    fn unsaved_imports_replace_disk() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("library.typ"), "#let stale() = [old]").unwrap();
        let mut sources = OpenSources::new(root);
        let library = open(&mut sources, root, "library.typ", "#let current() = [new]");
        let importer = open(
            &mut sources,
            root,
            "draft.typ",
            "#import \"library.typ\": current as local\n#local()",
        );
        let view = sources.view();
        let source = &view.get(&importer).unwrap().source;
        let names = source_only_graph(
            root,
            &view,
            source,
            None,
            &tola_typst::SourceBoundary::new(root, true),
            false,
            &mut DiskSources::default(),
            None,
            &BuildCancellation::new(),
        )
        .unwrap();
        let cursor = source.text().rfind("local").unwrap();
        let binding = names
            .selected(source.id(), cursor)
            .unwrap()
            .binding
            .unwrap();
        let importer_source = &view.get(&importer).unwrap().source;
        let uses: Vec<_> = names
            .references(binding, false)
            .into_iter()
            .filter(|(file, _)| *file == importer_source.id())
            .map(|(_, occurrence)| importer_source.text()[occurrence.range.clone()].to_owned())
            .collect();
        assert_eq!(uses, ["current", "local"]);
        assert_eq!(
            names.definition(binding).unwrap().0,
            view.get(&library).unwrap().source.id()
        );
    }

    /// References reach a consumer the client never opened: the dependents walk reads every
    /// Typst file the site holds — one the author's own ignore file lists included — and one
    /// consumer is neither open nor an import target of the file the request names.
    #[test]
    fn dependents_walk_reads_unopened_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::create_dir_all(root.join("modules")).unwrap();
        std::fs::write(root.join("modules/shared.typ"), "#let marker = 1\n").unwrap();
        std::fs::write(
            root.join("content/second.typ"),
            "#import \"../modules/shared.typ\": marker\n\nSecond, #marker.\n",
        )
        .unwrap();
        std::fs::write(root.join(".gitignore"), "content/second.typ\n").unwrap();
        let mut sources = OpenSources::new(root);
        let first = open(
            &mut sources,
            root,
            "content/first.typ",
            "#import \"../modules/shared.typ\": marker\n\nFirst, #marker.\n",
        );
        let view = sources.view();
        let source = &view.get(&first).unwrap().source;
        let cursor = source.text().rfind("marker").unwrap();
        let graph = source_only_graph(
            root,
            &view,
            source,
            None,
            &tola_typst::SourceBoundary::new(root, true),
            true,
            &mut DiskSources::default(),
            None,
            &BuildCancellation::new(),
        )
        .unwrap();
        let binding = graph
            .selected(source.id(), cursor)
            .unwrap()
            .binding
            .unwrap();
        let found: Vec<_> = graph
            .references(binding, true)
            .into_iter()
            .map(|(file, _)| file)
            .collect();
        assert_eq!(
            found.len(),
            5,
            "both imports, both uses, and the declaration: {found:?}"
        );
    }
}
