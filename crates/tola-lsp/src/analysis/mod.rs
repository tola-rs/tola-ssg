//! Protocol projection of the name graph into query replies.

mod disk;
mod graph;
mod selection;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{
    DocumentHighlight, DocumentHighlightKind, GotoDefinitionResponse, Location,
    PrepareRenameResponse, TextEdit, WorkspaceEdit,
};
use tola_typst::typst::syntax::{FileId, Source, VirtualRoot};
use tola_typst::{PackageFetchPolicy, PackageStore};
use tola_typst_syntax::imports::{BindingId, NameEdit, NameGraph, PendingImport};

/// Orders source identities by the site path the compiler addresses them under.
fn by_source_path(left: &FileId, right: &FileId) -> std::cmp::Ordering {
    left.vpath()
        .get_without_slash()
        .cmp(right.vpath().get_without_slash())
}

pub(crate) use disk::DiskSources;
pub(crate) use graph::{
    CachedGraph, GraphCache, GraphInputs, SourceOnlyGraph, file_resolver, graph, import_target,
    source_only_graph,
};
pub(crate) use selection::{site_selected_interfaces, site_sources};

use crate::protocol::{SourceQuery, SourceReply};

#[derive(Debug, thiserror::Error)]
pub(crate) enum NameQueryError {
    #[error("{0}")]
    Rename(tola_typst_syntax::imports::RenameError),
    #[error("{0}")]
    EntityRename(String),
    #[error(
        "this source has different text through multiple paths; save or close duplicate editor documents and try again"
    )]
    ConflictingSourceText,
    #[error(
        "cannot rename through `{0}` because its import targets depend on the path; use one explicit import target or separate the shared sources before renaming"
    )]
    ConflictingImportContexts(String),
}

impl NameQueryError {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::EntityRename(_) => "entityRenameRefused",
            Self::ConflictingSourceText => "conflictingSourceText",
            Self::ConflictingImportContexts(_) => "conflictingImportContexts",
            Self::Rename(error) => match error {
                tola_typst_syntax::imports::RenameError::InvalidIdentifier(_)
                | tola_typst_syntax::imports::RenameError::InvalidMathIdentifier(_) => {
                    "invalidIdentifier"
                }
                tola_typst_syntax::imports::RenameError::Conflict(_) => "renameConflict",
                tola_typst_syntax::imports::RenameError::UnknownImport(_) => "unknownImport",
                tola_typst_syntax::imports::RenameError::ReadOnly => "readOnly",
            },
        }
    }
}
/// One graph location as the editor receives it.
struct EditorLocation {
    location: Location,
    /// The first address is the one this location names; the rest hold the same source.
    addresses: Arc<[lsp_types::Uri]>,
}

/// Where this query's graph locations answer in the editor.
struct EditorLocations<'a> {
    graph: &'a NameGraph,
    source: &'a Source,
    query: &'a SourceQuery,
    view: &'a crate::sources::SourceView,
    source_root: &'a Path,
    client_root: &'a crate::uri::ClientRoot,
    packages: Option<PackageStore>,
    addresses_by_file: HashMap<FileId, Arc<[lsp_types::Uri]>>,
}

impl<'a> EditorLocations<'a> {
    /// The editor's addresses for one graph file, resolved once per query.
    fn addresses(&mut self, file: FileId, text: &str) -> Result<Option<Arc<[lsp_types::Uri]>>> {
        if let Some(addresses) = self.addresses_by_file.get(&file) {
            return Ok(Some(Arc::clone(addresses)));
        }
        let uri = if file == self.source.id() {
            self.query.document().0.clone()
        } else {
            let Some(uri) = package_directory_uri(
                file,
                self.source_root,
                self.client_root,
                self.packages.as_ref(),
            ) else {
                return Ok(None);
            };
            match crate::uri::to_site_path(uri.as_str())
                .ok()
                .and_then(|path| self.view.named_source(&path))
            {
                Some((opened, _)) => opened.parse().expect("tracked source URIs are valid"),
                None => uri,
            }
        };
        let mut addresses = Vec::new();
        if let Ok(path) = crate::uri::to_site_path(uri.as_str()) {
            require_source_text(self.view, &path, text)?;
            if matches!(self.query, SourceQuery::Rename(_)) {
                addresses.extend(
                    self.view
                        .versions_for_path(&path)
                        .map(|(opened, _)| opened.parse().expect("tracked source URIs are valid")),
                );
                if addresses.is_empty() {
                    addresses.push(self.client_root.address(&path)?);
                }
            }
        }
        if addresses.is_empty() {
            addresses.push(uri);
        }
        let addresses: Arc<[lsp_types::Uri]> = addresses.into();
        self.addresses_by_file.insert(file, Arc::clone(&addresses));
        Ok(Some(addresses))
    }

    /// One graph location in the editor's spelling, when the editor addresses its file at all.
    fn locate(&mut self, file: FileId, bytes: Range<usize>) -> Result<Option<EditorLocation>> {
        let graph = self.graph;
        let target = graph
            .source(file)
            .expect("a graph location belongs to a source")
            .source();
        let Some(addresses) = self.addresses(file, target.text())? else {
            return Ok(None);
        };
        let Some(range) = crate::position::utf16_range(target.lines(), bytes) else {
            return Ok(None);
        };
        Ok(Some(EditorLocation {
            location: Location {
                uri: addresses[0].clone(),
                range,
            },
            addresses,
        }))
    }
}

/// One name-graph answer's graph, source, client view, and resolution inputs.
pub(crate) struct NameGraphReply<'a> {
    pub(crate) graph: &'a NameGraph,
    pub(crate) source: &'a Source,
    pub(crate) cursor: usize,
    pub(crate) query: &'a SourceQuery,
    pub(crate) view: &'a crate::sources::SourceView,
    pub(crate) disk: &'a DiskSources,
    pub(crate) source_root: &'a Path,
    pub(crate) client_root: &'a crate::uri::ClientRoot,
    pub(crate) locations: Option<&'a tola_typst::PackageLocations>,
    pub(crate) boundary: &'a tola_typst::SourceBoundary,
}

pub(crate) fn respond(inputs: NameGraphReply<'_>) -> Result<Option<SourceReply>> {
    let NameGraphReply {
        graph,
        source,
        cursor,
        query,
        view,
        disk,
        source_root,
        client_root,
        locations,
        boundary,
    } = inputs;
    if !query.answered_from_name_graph() {
        return Ok(None);
    }
    let Some(selected) = graph.selected(source.id(), cursor) else {
        return Ok(None);
    };
    let Some(binding) = selected.binding else {
        return Ok((!matches!(query, SourceQuery::Definition(_)))
            .then(|| crate::query::unavailable(query)));
    };
    let packages = locations.map(|locations| {
        PackageStore::new(locations.clone(), PackageFetchPolicy::LocalOnly)
            .with_source_boundary(boundary.clone())
    });
    let mut located = EditorLocations {
        graph,
        source,
        query,
        view,
        source_root,
        client_root,
        packages,
        addresses_by_file: HashMap::new(),
    };
    Ok(match query {
        SourceQuery::Definition(_) => definition_reply(&mut located, binding)?,
        SourceQuery::PrepareRename(_) => Some(prepare_rename_reply(
            graph,
            source,
            selected.range.clone(),
            binding,
        )),
        SourceQuery::References(params) => {
            Some(references_reply(&mut located, disk, binding, params)?)
        }
        SourceQuery::DocumentHighlights(_) => Some(highlights_reply(graph, source, binding)),
        SourceQuery::Rename(params) => {
            Some(rename_reply(&mut located, disk, binding, &params.new_name)?)
        }
        _ => unreachable!("the name graph answers only the queries handled above"),
    })
}

/// The definition one binding resolves to, or no reply when the graph establishes none.
fn definition_reply(
    located: &mut EditorLocations<'_>,
    binding: BindingId,
) -> Result<Option<SourceReply>> {
    let Some((file, bytes)) = located.graph.definition(binding) else {
        // The compiler can still establish a definition for an import this graph never resolved.
        return Ok(None);
    };
    let definition = located
        .locate(file, bytes)?
        .map(|EditorLocation { location, .. }| GotoDefinitionResponse::Scalar(location));
    Ok(Some(SourceReply::Definition(definition)))
}

/// The range and placeholder a rename would edit, when the editor may write the declaration.
fn prepare_rename_reply(
    graph: &NameGraph,
    source: &Source,
    spelling: Range<usize>,
    binding: BindingId,
) -> SourceReply {
    let declared = graph.binding(binding);
    let range = matches!(declared.file.root(), VirtualRoot::Project)
        .then(|| crate::position::utf16_range(source.lines(), spelling))
        .flatten();
    SourceReply::PrepareRename(
        range.map(|range| PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: declared.name.clone(),
        }),
    )
}

/// Every location one spelling's bindings answer, in protocol order.
fn references_reply(
    located: &mut EditorLocations<'_>,
    disk: &DiskSources,
    binding: BindingId,
    params: &lsp_types::ReferenceParams,
) -> Result<SourceReply> {
    let graph = located.graph;
    let root = tola_build::filesystem::normalize_existing_prefix(located.source_root);
    let declaration = graph.definition(binding);
    let mut references = Vec::new();
    for binding in declaration_bindings(located, disk, &root, binding, declaration)? {
        for (file, occurrence) in graph.references(binding, params.context.include_declaration) {
            if let Some(EditorLocation { location, .. }) =
                located.locate(file, occurrence.range.clone())?
            {
                references.push(location);
            }
        }
    }
    references.sort_by(|left, right| {
        (left.uri.as_str(), left.range.start, left.range.end).cmp(&(
            right.uri.as_str(),
            right.range.start,
            right.range.end,
        ))
    });
    references.dedup();
    Ok(SourceReply::References(Some(references)))
}

/// Every occurrence of one binding in the queried document.
fn highlights_reply(graph: &NameGraph, source: &Source, binding: BindingId) -> SourceReply {
    let highlights = graph
        .occurrences(source.id())
        .iter()
        .filter(|occurrence| occurrence.binding == Some(binding))
        .filter_map(|occurrence| {
            Some(DocumentHighlight {
                range: crate::position::utf16_range(source.lines(), occurrence.range.clone())?,
                kind: Some(if occurrence.declaration {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                }),
            })
        })
        .collect();
    SourceReply::DocumentHighlights(Some(highlights))
}

/// The workspace edit that renames one spelling through every binding it covers.
fn rename_reply(
    located: &mut EditorLocations<'_>,
    disk: &DiskSources,
    binding: BindingId,
    new_name: &str,
) -> Result<SourceReply> {
    let graph = located.graph;
    let root = tola_build::filesystem::normalize_existing_prefix(located.source_root);
    let declared = graph.binding(binding);
    let declaration = declared.declaration.map(|declaration| {
        let source = graph
            .source(declared.file)
            .expect("a binding belongs to a graph source");
        (
            declared.file,
            source.declarations()[declaration].range.clone(),
        )
    });
    let mut edits = Vec::new();
    for binding in declaration_bindings(located, disk, &root, binding, declaration)? {
        edits.extend(
            graph
                .rename(binding, new_name)
                .map_err(NameQueryError::Rename)?,
        );
    }
    validate_aliased_edits(graph, disk, &root, &edits)?;
    Ok(SourceReply::Rename(Some(WorkspaceEdit {
        changes: Some(rename_changes(located, edits)?),
        ..WorkspaceEdit::default()
    })))
}

/// The rename edits keyed by the URI each lands in; each URI's list is in protocol order.
fn rename_changes(
    located: &mut EditorLocations<'_>,
    edits: Vec<NameEdit>,
) -> Result<HashMap<lsp_types::Uri, Vec<TextEdit>>> {
    let mut changes = HashMap::new();
    for edit in edits {
        let EditorLocation {
            location,
            addresses,
        } = located
            .locate(edit.file, edit.range)?
            .expect("writable graph sources have document URIs");
        let edit = TextEdit {
            range: location.range,
            new_text: edit.replacement,
        };
        for uri in addresses.iter().skip(1) {
            changes
                .entry(uri.clone())
                .or_insert_with(Vec::new)
                .push(edit.clone());
        }
        changes
            .entry(location.uri)
            .or_insert_with(Vec::new)
            .push(edit);
    }
    for edits in changes.values_mut() {
        edits.sort_by(|left, right| {
            (left.range.start, left.range.end, &left.new_text).cmp(&(
                right.range.start,
                right.range.end,
                &right.new_text,
            ))
        });
        edits.dedup();
    }
    Ok(changes)
}

/// The bindings a reference or rename covers: the spelling's own, plus every logical instance.
fn declaration_bindings(
    located: &mut EditorLocations<'_>,
    disk: &DiskSources,
    root: &Path,
    binding: BindingId,
    declaration: Option<(FileId, Range<usize>)>,
) -> Result<Vec<BindingId>> {
    let graph = located.graph;
    let mut bindings = vec![binding];
    if let Some((file, bytes)) = declaration {
        // Reading the declaration is what checks its text against the editor's buffer.
        located.locate(file, bytes.clone())?;
        bindings.extend(source_bindings(graph, disk, file, bytes.start, root)?);
    }
    bindings.sort_unstable();
    bindings.dedup();
    Ok(bindings)
}

/// Every logical instance of a physical consumer must license the same text changes.
fn validate_aliased_edits(
    graph: &NameGraph,
    disk: &DiskSources,
    root: &Path,
    edits: &[NameEdit],
) -> Result<(), NameQueryError> {
    let mut rewrites: HashMap<_, HashSet<_>> = HashMap::new();
    for edit in edits {
        rewrites.entry(edit.file).or_default().insert((
            edit.range.start,
            edit.range.end,
            edit.replacement.as_str(),
        ));
    }
    let mut files: Vec<_> = rewrites.keys().copied().collect();
    files.sort_by(by_source_path);
    for file in files {
        for alias in disk.source_aliases(file, root) {
            if alias == file {
                continue;
            }
            let Some(counterpart) = graph.source(alias) else {
                continue;
            };
            if counterpart.source().text()
                != graph
                    .source(file)
                    .expect("a rename edits a graph source")
                    .source()
                    .text()
            {
                return Err(NameQueryError::ConflictingSourceText);
            }
            if rewrites.get(&alias) != rewrites.get(&file) {
                return Err(NameQueryError::ConflictingImportContexts(
                    file.vpath().get_without_slash().to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn require_source_text(
    view: &crate::sources::SourceView,
    path: &Path,
    text: &str,
) -> Result<(), NameQueryError> {
    if view
        .overrides()
        .iter()
        .any(|(opened, current)| opened == path && current.as_ref() != text)
    {
        return Err(NameQueryError::ConflictingSourceText);
    }
    Ok(())
}

/// Bindings of one physical declaration, with each module's logical import base kept intact.
fn source_bindings(
    graph: &NameGraph,
    disk: &DiskSources,
    file: FileId,
    at: usize,
    root: &Path,
) -> Result<Vec<BindingId>, NameQueryError> {
    if !matches!(file.root(), VirtualRoot::Project) {
        return Ok(Vec::new());
    }
    let source = graph
        .source(file)
        .expect("a definition belongs to a graph source");
    let mut bindings = Vec::new();
    for id in disk.source_aliases(file, root) {
        let Some(names) = graph.source(id) else {
            continue;
        };
        if names.source().text() != source.source().text() {
            return Err(NameQueryError::ConflictingSourceText);
        }
        if let Some(binding) = graph.selected(id, at).and_then(|selected| selected.binding) {
            bindings.push(binding);
        }
    }
    Ok(bindings)
}

/// The statements whose official target this query still needs before its answer is complete.
///
/// References and rename cover the consumers a declaration's export reaches, which only the
/// official evaluation proves. A definition, prepare-rename, and highlights read the spelling's
/// own identity: they need the proof only when the graph cannot establish it, which is what an
/// import expression whose interface the graph never resolved leaves. A spelling the graph binds
/// to nothing is answered the same way, because a wildcard statement it never resolved may be
/// what brings the name in.
pub(crate) fn unproven(
    graph: &NameGraph,
    source: &Source,
    cursor: usize,
    query: &SourceQuery,
) -> Vec<PendingImport> {
    if !query.answered_from_name_graph() {
        return Vec::new();
    }
    match query {
        SourceQuery::References(_) | SourceQuery::Rename(_) => {
            graph.unresolved_imports(source.id(), cursor)
        }
        _ => match graph
            .selected(source.id(), cursor)
            .map(|selected| selected.binding)
        {
            Some(Some(binding)) => graph.unresolved_item(binding).into_iter().collect(),
            Some(None) => graph.unresolved_imports(source.id(), cursor),
            None => Vec::new(),
        },
    }
}

/// The address of one file whose package directory this lane prepared, in the spelling the editor
/// named the root with.
///
/// A package this implementation does not embed is addressed where the compiler resolved it, so a
/// definition into it opens; every other file answers through [`crate::identity::client_uri`].
fn package_directory_uri(
    file: FileId,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
    packages: Option<&PackageStore>,
) -> Option<lsp_types::Uri> {
    if let VirtualRoot::Package(spec) = file.root()
        && crate::identity::embedded_source(file).is_none()
    {
        let package = packages?.prepare(spec).ok()?;
        return crate::uri::from_file_path(
            &package.directory().join(file.vpath().get_without_slash()),
        )
        .ok();
    }
    crate::identity::client_uri(file, source_root, client_root).ok()
}
