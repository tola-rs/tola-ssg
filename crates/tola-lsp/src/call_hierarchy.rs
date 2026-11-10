//! The document graph a site's sources form, as the protocol's call hierarchy.
//!
//! rust-analyzer's call hierarchy is the protocol shape; a callable here is a source rather than a
//! function. Typst markup has no call graph, but a site's sources do form one: a source `#include`s
//! or `#import`s another, and the pages a source is served at are what an author recognises it by.
//! Every divergence follows from that mapping: an item names a file rather than a declaration, its
//! range is the file's own start rather than a declaration's, and the pages a caller serves are its
//! detail rather than its container.
//!
//! A node's callers are the sources that reach it, by either relation:
//!
//! - the pages whose body has the node, which a check proved by realizing a document from
//!   them. This sees a `#include` whose path only evaluation establishes;
//! - the sources that write the node's path in an `#include` or a literal `#import`, which is what
//!   places a caller's range inside the caller.
//!
//! A path only a value can establish — `#include some-record.file` in the site program — reaches a
//! page through the realized document rather than through the path, so it names a caller without a
//! range. A node no source reaches answers with nothing rather than failing.
//!
//! A node's callees are the sources its own literal paths reach: the mirror of the second caller
//! relation. A path only a value can establish reaches nothing here, and a path that names no site
//! file — a package, or one outside the root — reaches nothing either.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use crate::protocol::Route;
use crate::sentence::listed;
use crate::sources::SourceView;
use anyhow::Result;
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position,
    Range as ProtocolRange, SymbolKind,
};
use serde::{Deserialize, Serialize};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::syntax::{FileId, Source, VirtualRoot};
use tola_typst::{PackageLocations, SourceBoundary};
use tola_typst_syntax::syntax::module_paths;

/// What one call-hierarchy item names.
///
/// The protocol has this back in the item's `data`, so it is the whole identity an
/// `incomingCalls` request has of the item it asks about.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum ItemNode {
    /// A source of the site, by the site-relative path a client resolves it as.
    Source { path: String },
}

impl ItemNode {
    fn of(source: FileId) -> Self {
        Self::Source {
            path: source.vpath().get_without_slash().to_owned(),
        }
    }
}

/// The file one item's `data` names, when it names a site file.
///
/// `root` may be spelled as the client named it: the path the data has is compared through
/// the normalized root, as every site path compared in the crate is.
pub(super) fn item_file(data: &serde_json::Value, root: &Path) -> Option<FileId> {
    let ItemNode::Source { path } = serde_json::from_value(data.clone()).ok()?;
    let root = tola_build::filesystem::normalize_existing_prefix(root);
    crate::identity::path_id(&root.join(path), &root)
}

/// The item the cursor names: the source an `#include` or `#import` path writes, or the file the
/// cursor is in.
///
/// This reads the editor's own text: an item's range is a file's start rather than a declaration,
/// so the site's compilation is not touched.
pub(super) fn prepare(
    root: &Path,
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
    source: &Source,
    cursor: usize,
    client_root: &crate::uri::ClientRoot,
) -> Option<CallHierarchyItem> {
    let resolver = crate::analysis::file_resolver(locations, boundary);
    let named = module_paths(source)
        .into_iter()
        .find(|path| path.range.start <= cursor && cursor <= path.range.end)
        .and_then(|path| {
            crate::analysis::import_target(source.id(), &path.text, root, resolver.as_ref())
        })
        .filter(|id| matches!(id.root(), VirtualRoot::Project));
    source_item(root, named.unwrap_or(source.id()), None, client_root)
}

/// One incoming-calls read: the roots, view, and compiled revision the answer reads.
pub(super) struct Incoming<'a> {
    pub(super) disk: &'a mut crate::analysis::DiskSources,
    pub(super) root: &'a Path,
    pub(super) view: &'a SourceView,
    pub(super) locations: Option<&'a PackageLocations>,
    pub(super) boundary: &'a SourceBoundary,
    pub(super) config: &'a ResolvedSiteConfig,
    pub(super) compilation: &'a SourceCompilation,
    pub(super) source: &'a Source,
    pub(super) cancellation: &'a BuildCancellation,
    pub(super) client_root: &'a crate::uri::ClientRoot,
}

/// Every source that reaches `source`, with the ranges inside each that write its path.
///
/// Sources resolve through `root`; item URIs keep the spelling `client_root` maps them to.
pub(super) fn incoming(inputs: Incoming<'_>) -> Result<Vec<CallHierarchyIncomingCall>> {
    let Incoming {
        disk,
        root,
        view,
        locations,
        boundary,
        config,
        compilation,
        source,
        cancellation,
        client_root,
    } = inputs;
    cancellation.ensure_active()?;
    let resolver = crate::analysis::file_resolver(locations, boundary);
    let resolved = tola_build::filesystem::normalize_existing_prefix(root);
    let mut reaching: BTreeMap<String, Reaching> = BTreeMap::new();
    for page in crate::routes::pages_carrying(compilation, config, source.id(), cancellation)? {
        let Some(writer) = page.source else {
            continue;
        };
        if writer == source.id() {
            continue;
        }
        // The page has the node, so it reaches it even when only evaluation established the
        // path; a source that also writes the path is found below and keeps its own ranges.
        reaching
            .entry(writer.vpath().get_without_slash().to_owned())
            .or_insert_with(|| Reaching {
                source: writer,
                ranges: Vec::new(),
                detail: served_at(std::slice::from_ref(&page.route)),
            });
    }
    let graph = crate::analysis::source_only_graph(crate::analysis::SourceOnlyGraph {
        root: &resolved,
        view,
        source,
        locations,
        boundary,
        include_dependents: true,
        disk,
        cache: None,
        cancellation,
    })?;
    for names in graph.sources() {
        cancellation.ensure_active()?;
        let writer = names.source().id();
        if writer == source.id() || !matches!(writer.root(), VirtualRoot::Project) {
            continue;
        }
        let mut ranges: Vec<Range<usize>> = module_paths(names.source())
            .into_iter()
            .filter(|path| {
                crate::analysis::import_target(writer, &path.text, &resolved, resolver.as_ref())
                    == Some(source.id())
            })
            .map(|path| path.range)
            .collect();
        if ranges.is_empty() {
            continue;
        }
        ranges.sort_by_key(|range| range.start);
        let from = ranges
            .iter()
            .filter_map(|range| crate::position::utf16_range(names.source().lines(), range.clone()))
            .collect();
        let pages = crate::routes::pages_carrying(compilation, config, writer, cancellation)?
            .into_iter()
            .map(|page| page.route)
            .collect::<Vec<Route>>();
        reaching.insert(
            writer.vpath().get_without_slash().to_owned(),
            Reaching {
                source: writer,
                ranges: from,
                detail: served_at(&pages),
            },
        );
    }
    Ok(reaching
        .into_values()
        .filter_map(|reaching| {
            Some(CallHierarchyIncomingCall {
                from: source_item(root, reaching.source, reaching.detail, client_root)?,
                from_ranges: reaching.ranges,
            })
        })
        .collect())
}

/// Every source `source` reaches, with the ranges inside it that write their paths.
///
/// The mirror of `incoming`: a literal `#include` or `#import` path names the file it reaches, so
/// the ranges inside `source` are where the call happens. A path only a value can establish names
/// nothing until evaluation, so the relation it establishes is visible only in `incoming`, through
/// the realized document.
pub(super) fn outgoing(
    root: &Path,
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
    source: &Source,
    client_root: &crate::uri::ClientRoot,
) -> Vec<CallHierarchyOutgoingCall> {
    let resolver = crate::analysis::file_resolver(locations, boundary);
    let resolved = tola_build::filesystem::normalize_existing_prefix(root);
    let mut reached: BTreeMap<String, Target> = BTreeMap::new();
    for written in module_paths(source) {
        let Some(target) = crate::analysis::import_target(
            source.id(),
            &written.text,
            &resolved,
            resolver.as_ref(),
        ) else {
            continue;
        };
        if !matches!(target.root(), VirtualRoot::Project) {
            continue;
        }
        let Some(range) = crate::position::utf16_range(source.lines(), written.range) else {
            continue;
        };
        reached
            .entry(target.vpath().get_without_slash().to_owned())
            .and_modify(|reached| reached.ranges.push(range))
            .or_insert_with(|| Target {
                source: target,
                ranges: vec![range],
            });
    }
    reached
        .into_values()
        .filter_map(|reached| {
            Some(CallHierarchyOutgoingCall {
                to: source_item(root, reached.source, None, client_root)?,
                from_ranges: reached.ranges,
            })
        })
        .collect()
}

/// One source that reaches the node, and the ranges inside it that write the node's path.
struct Reaching {
    source: FileId,
    ranges: Vec<ProtocolRange>,
    detail: Option<String>,
}

/// One source `source` reaches, with the ranges inside `source` that write the target's path.
struct Target {
    source: FileId,
    ranges: Vec<ProtocolRange>,
}

/// The item naming one source, with the pages it is served at as its detail.
fn source_item(
    root: &Path,
    source: FileId,
    detail: Option<String>,
    client_root: &crate::uri::ClientRoot,
) -> Option<CallHierarchyItem> {
    Some(CallHierarchyItem {
        name: source.vpath().get_without_slash().to_owned(),
        kind: SymbolKind::FILE,
        tags: None,
        detail,
        uri: crate::identity::client_uri(source, root, client_root).ok()?,
        range: file_range(),
        selection_range: file_range(),
        data: Some(serde_json::to_value(ItemNode::of(source)).expect("item identity is JSON")),
    })
}

/// The pages a caller is served at, as its detail line.
///
/// A source many pages include names its first few and one entry stands for the rest, exactly as
/// the same source's lenses name them.
fn served_at(routes: &[Route]) -> Option<String> {
    if routes.is_empty() {
        return None;
    }
    let (mut listed, remaining) = listed(
        routes.iter().map(|route| route.route.as_str()),
        crate::routes::NAMED_PAGES,
    );
    if remaining > 0 {
        listed.push_str(&format!(" and {remaining} more"));
    }
    Some(format!("Served at {listed}"))
}

/// The range an item denoting a whole file has.
///
/// A source has no declaration to select, so a client opening such an item lands at the file's
/// start.
fn file_range() -> ProtocolRange {
    ProtocolRange::new(Position::new(0, 0), Position::new(0, 0))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tola_build::check::{SourceDiagnosticSession, SourceRevision};
    use tola_diagnostic::Severity;

    use super::*;
    use crate::server::tests::rendering_site;

    /// A site that compiled, with the revision every question is asked against.
    fn compiled_site(
        files: &[(&str, &str)],
    ) -> (
        tempfile::TempDir,
        Arc<ResolvedSiteConfig>,
        BuildCancellation,
        SourceRevision,
    ) {
        let (directory, configuration) = rendering_site(files);
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::new(Arc::clone(&configuration));
        let revision = session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            revision
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{:#?}",
            revision.diagnostics()
        );
        (directory, configuration, cancellation, revision)
    }

    /// One file of the site, as the compiler identifies it.
    fn source(root: &Path, name: &str) -> Source {
        let path = root.join(name);
        Source::new(
            crate::identity::path_id(&path, root).expect("the file has a site identity"),
            std::fs::read_to_string(&path).unwrap(),
        )
    }

    /// The calls `name` answers as an incoming target, over a compiled revision.
    fn incoming_for(
        configuration: &Arc<ResolvedSiteConfig>,
        revision: &SourceRevision,
        cancellation: &BuildCancellation,
        name: &str,
    ) -> Vec<CallHierarchyIncomingCall> {
        let root = configuration.get_root();
        incoming(Incoming {
            disk: &mut crate::analysis::DiskSources::default(),
            root,
            view: &SourceView::default(),
            locations: None,
            boundary: &crate::sources::base_source_boundary(root, true),
            config: configuration,
            compilation: revision.checked().expect("the site compiles"),
            source: &source(root, name),
            cancellation,
            client_root: &crate::uri::ClientRoot::new(root),
        })
        .unwrap()
    }

    /// The names and details of the calls one request answers with.
    fn named_calls(calls: &[CallHierarchyIncomingCall]) -> Vec<(String, Option<String>)> {
        calls
            .iter()
            .map(|call| (call.from.name.clone(), call.from.detail.clone()))
            .collect()
    }

    #[test]
    fn incoming_names_the_including_pages() {
        let (_directory, configuration, cancellation, revision) = compiled_site(&[
            (
                "content/one.typ",
                "#include \"../templates/note.typ\"\nOne.\n",
            ),
            (
                "content/two.typ",
                "Intro.\n#include \"../templates/note.typ\"\nTwo.\n",
            ),
            ("templates/note.typ", "Note.\n"),
        ]);
        let found = incoming_for(
            &configuration,
            &revision,
            &cancellation,
            "templates/note.typ",
        );

        assert_eq!(
            named_calls(&found),
            [
                (
                    "content/one.typ".to_owned(),
                    Some("Served at /one/".to_owned())
                ),
                (
                    "content/two.typ".to_owned(),
                    Some("Served at /two/".to_owned())
                ),
            ]
        );
        assert_eq!(
            found[0].from_ranges,
            [ProtocolRange::new(
                Position::new(0, 9),
                Position::new(0, 32)
            )]
        );
        assert_eq!(
            found[1].from_ranges,
            [ProtocolRange::new(
                Position::new(1, 9),
                Position::new(1, 32)
            )]
        );
    }

    #[test]
    fn incoming_names_the_importing_source() {
        let (_directory, configuration, cancellation, revision) = compiled_site(&[
            (
                "content/one.typ",
                "#import \"../templates/lib.typ\": greet\n#greet()\n",
            ),
            ("templates/lib.typ", "#let greet() = [hello]\n"),
        ]);
        let found = incoming_for(
            &configuration,
            &revision,
            &cancellation,
            "templates/lib.typ",
        );

        assert_eq!(
            named_calls(&found),
            [(
                "content/one.typ".to_owned(),
                Some("Served at /one/".to_owned())
            )]
        );
        assert_eq!(
            found[0].from_ranges,
            [ProtocolRange::new(
                Position::new(0, 8),
                Position::new(0, 30)
            )]
        );
    }
    #[test]
    fn outgoing_names_reached_files() {
        let (_directory, configuration) = rendering_site(&[
            (
                "content/document.typ",
                "#include \"note.typ\"\n#import \"lib.typ\": greet\n",
            ),
            ("content/note.typ", "Note.\n"),
            ("content/lib.typ", "#let greet() = [hi]\n"),
        ]);
        let root = configuration.get_root();
        let source = source(root, "content/document.typ");

        let found = outgoing(
            root,
            None,
            &crate::sources::base_source_boundary(root, true),
            &source,
            &crate::uri::ClientRoot::new(root),
        );

        assert_eq!(
            found
                .iter()
                .map(|call| (call.to.name.clone(), call.from_ranges.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    "content/lib.typ".to_owned(),
                    vec![ProtocolRange::new(
                        Position::new(1, 8),
                        Position::new(1, 17)
                    )]
                ),
                (
                    "content/note.typ".to_owned(),
                    vec![ProtocolRange::new(
                        Position::new(0, 9),
                        Position::new(0, 19)
                    )]
                ),
            ]
        );
    }
    #[test]
    fn prepare_names_the_written_source() {
        let (_directory, configuration) = rendering_site(&[(
            "content/one.typ",
            "#include \"../templates/note.typ\"\nOne.\n",
        )]);
        let root = configuration.get_root();
        let source = source(root, "content/one.typ");

        let item = prepare(
            root,
            None,
            &crate::sources::base_source_boundary(root, true),
            &source,
            14,
            &crate::uri::ClientRoot::new(root),
        )
        .expect("an item");

        assert_eq!(item.name, "templates/note.typ");
        assert_eq!(
            item_file(item.data.as_ref().expect("an identity"), root),
            crate::identity::path_id(&root.join("templates/note.typ"), root)
        );
        assert_eq!(
            item.data,
            Some(json!({"kind": "source", "path": "templates/note.typ"}))
        );
    }
}
