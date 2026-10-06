//! Protocol projection of the name graph into query replies.

mod disk;
mod graph;
mod selection;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use lsp_types::{
    DocumentHighlight, DocumentHighlightKind, GotoDefinitionResponse, Location,
    PrepareRenameResponse, TextEdit, WorkspaceEdit,
};
use tola_typst::typst::syntax::{FileId, Source, VirtualRoot};
use tola_typst::{PackageFetchPolicy, PackageStore};
use tola_typst_syntax::imports::{NameGraph, PendingImport};

pub(crate) use disk::DiskSources;
pub(crate) use graph::{
    CachedGraph, GraphCache, file_resolver, graph, import_target, source_only_graph,
};
pub(crate) use selection::site_selected_interfaces;

use crate::protocol::{SourceQuery, SourceReply};

#[derive(Debug)]
pub(crate) struct RenameError(tola_typst_syntax::imports::RenameError);

impl RenameError {
    pub(crate) fn kind(&self) -> &'static str {
        match self.0 {
            tola_typst_syntax::imports::RenameError::InvalidIdentifier(_)
            | tola_typst_syntax::imports::RenameError::InvalidMathIdentifier(_) => {
                "invalidIdentifier"
            }
            tola_typst_syntax::imports::RenameError::Conflict(_) => "renameConflict",
            tola_typst_syntax::imports::RenameError::UnknownImport(_) => "unknownImport",
            tola_typst_syntax::imports::RenameError::ReadOnly => "readOnly",
        }
    }
}
impl std::fmt::Display for RenameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl std::error::Error for RenameError {}

pub(crate) fn respond(
    graph: &NameGraph,
    source: &Source,
    cursor: usize,
    query: &SourceQuery,
    client_root: &Path,
    locations: Option<&tola_typst::PackageLocations>,
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<SourceReply>> {
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
    // The callers pass the root in the spelling the client named; a file is addressed by that
    // spelling rejoined, never the resolved form the compiler reads it through.
    let client_root = crate::uri::ClientRoot::new(client_root);
    let mut uris: HashMap<FileId, lsp_types::Uri> = HashMap::new();
    let mut location = |file, range| -> Option<Location> {
        let uri = if let Some(uri) = uris.get(&file) {
            uri.clone()
        } else {
            // A package document answers under its own identity, whichever spelling the client
            // used to reach it: a mirror file and the `tola-package:` document it mirrors are one
            // document, so their answers never disagree.
            let uri = if file == source.id() && !matches!(file.root(), VirtualRoot::Package(_)) {
                query.document().0.clone()
            } else {
                package_directory_uri(file, &client_root, packages.as_ref())?
            };
            uris.insert(file, uri.clone());
            uri
        };
        Some(Location {
            uri,
            range: crate::position::utf16_range(graph.source(file)?.source().lines(), range)?,
        })
    };
    Ok(Some(match query {
        SourceQuery::Definition(_) => {
            let Some((file, range)) = graph.definition(binding) else {
                // The name graph has no definition; the compiler can still establish one, as it
                // does for an import expression whose interface the graph never resolved.
                return Ok(None);
            };
            SourceReply::Definition(location(file, range).map(GotoDefinitionResponse::Scalar))
        }
        SourceQuery::PrepareRename(_) => {
            let writable = matches!(graph.binding(binding).file.root(), VirtualRoot::Project);
            SourceReply::PrepareRename(
                writable
                    .then(|| {
                        Some(PrepareRenameResponse::RangeWithPlaceholder {
                            range: crate::position::utf16_range(
                                source.lines(),
                                selected.range.clone(),
                            )?,
                            placeholder: graph.binding(binding).name.clone(),
                        })
                    })
                    .flatten(),
            )
        }
        SourceQuery::References(params) => SourceReply::References(Some(
            graph
                .references(binding, params.context.include_declaration)
                .into_iter()
                .filter_map(|(file, occurrence)| location(file, occurrence.range.clone()))
                .collect(),
        )),
        SourceQuery::DocumentHighlights(_) => SourceReply::DocumentHighlights(Some(
            graph
                .occurrences(source.id())
                .iter()
                .filter(|occurrence| occurrence.binding == Some(binding))
                .filter_map(|occurrence| {
                    Some(DocumentHighlight {
                        range: crate::position::utf16_range(
                            source.lines(),
                            occurrence.range.clone(),
                        )?,
                        kind: Some(if occurrence.declaration {
                            DocumentHighlightKind::WRITE
                        } else {
                            DocumentHighlightKind::READ
                        }),
                    })
                })
                .collect(),
        )),
        SourceQuery::Rename(params) => {
            let edits = graph
                .rename(binding, &params.new_name)
                .map_err(RenameError)?;
            #[expect(
                clippy::mutable_key_type,
                reason = "the protocol keys workspace edits by URI"
            )]
            let mut changes = HashMap::new();
            for edit in edits {
                if let Some(location) = location(edit.file, edit.range) {
                    changes
                        .entry(location.uri)
                        .or_insert_with(Vec::new)
                        .push(TextEdit {
                            range: location.range,
                            new_text: edit.replacement,
                        });
                }
            }
            SourceReply::Rename(Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }))
        }
        _ => unreachable!(),
    }))
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
    crate::identity::client_uri(file, client_root)
}
