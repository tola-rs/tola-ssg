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
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, SyntaxKind, VirtualRoot};
use tola_typst::{PackageLocations, SourceBoundary};

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
) -> Option<CallHierarchyItem> {
    let resolver = crate::analysis::file_resolver(locations, boundary);
    let named = written_paths(source)
        .into_iter()
        .find(|path| path.range.start <= cursor && cursor <= path.range.end)
        .and_then(|path| {
            crate::analysis::import_target(source.id(), &path.text, root, resolver.as_ref())
        })
        .filter(|id| matches!(id.root(), VirtualRoot::Project));
    source_item(root, named.unwrap_or(source.id()), None)
}

/// Every source that reaches `source`, with the ranges inside each that write its path.
///
/// `root` is the root the client named, so items keep that spelling; the sources the graph is
/// built from resolve through it, as every site path compared in the crate does.
#[expect(
    clippy::too_many_arguments,
    reason = "the incoming read takes disk, root, view, locations, boundary, config, compilation, source, and cancellation"
)]
pub(super) fn incoming(
    disk: &mut crate::analysis::DiskSources,
    root: &Path,
    view: &SourceView,
    locations: Option<&PackageLocations>,
    boundary: &SourceBoundary,
    config: &ResolvedSiteConfig,
    compilation: &SourceCompilation,
    source: &Source,
    cancellation: &BuildCancellation,
) -> Result<Vec<CallHierarchyIncomingCall>> {
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
    let graph = crate::analysis::source_only_graph(
        &resolved,
        view,
        source,
        locations,
        boundary,
        true,
        disk,
        None,
        cancellation,
    )?;
    for names in graph.sources() {
        cancellation.ensure_active()?;
        let writer = names.source().id();
        if writer == source.id() || !matches!(writer.root(), VirtualRoot::Project) {
            continue;
        }
        let mut ranges: Vec<Range<usize>> = written_paths(names.source())
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
                from: source_item(root, reaching.source, reaching.detail)?,
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
) -> Vec<CallHierarchyOutgoingCall> {
    let resolver = crate::analysis::file_resolver(locations, boundary);
    let resolved = tola_build::filesystem::normalize_existing_prefix(root);
    let mut reached: BTreeMap<String, Target> = BTreeMap::new();
    for written in written_paths(source) {
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
                to: source_item(root, reached.source, None)?,
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

/// One `#include` or `#import` path a source writes.
struct WrittenPath {
    /// The path the statement decodes to, which is what the compiler resolves.
    text: String,
    /// The statement's bytes, quotes included.
    range: Range<usize>,
}

/// The paths one source writes: every `#include` and every `#import` with a literal path.
///
/// A statement whose path is an expression names no file until evaluation, so it writes no path
/// this graph can resolve.
fn written_paths(source: &Source) -> Vec<WrittenPath> {
    let mut paths = Vec::new();
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        // The statement's own untied node has the tree's lifetime, so the path it names
        // outlives this walk; a name cast from an owned child borrows the child.
        let literal = match node.kind() {
            SyntaxKind::ModuleInclude => node
                .get()
                .children()
                .find(|child| child.kind() == SyntaxKind::Str)
                .and_then(|child| child.cast::<ast::Str>()),
            SyntaxKind::ModuleImport => {
                node.get()
                    .cast::<ast::ModuleImport>()
                    .and_then(|import| match import.source() {
                        ast::Expr::Str(text) => Some(text),
                        _ => None,
                    })
            }
            _ => {
                stack.extend(node.children());
                continue;
            }
        };
        if let Some(text) = literal
            && let Some(written) = node.find(text.span())
        {
            let range = written.range();
            // A string the author has not closed yet is one quotation mark, and reading its value
            // would cut both ends off a single byte.
            if range.end >= range.start + 2 {
                paths.push(WrittenPath {
                    text: text.get().to_string(),
                    range,
                });
            }
        }
    }
    paths.sort_by_key(|path| path.range.start);
    paths
}

/// The item naming one source, with the pages it is served at as its detail.
fn source_item(root: &Path, source: FileId, detail: Option<String>) -> Option<CallHierarchyItem> {
    Some(CallHierarchyItem {
        name: source.vpath().get_without_slash().to_owned(),
        kind: SymbolKind::FILE,
        tags: None,
        detail,
        uri: crate::identity::source_uri(source, root).ok()?,
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
    use tola_build::config::loading::{BuildOverrides, load_site_config};
    use tola_build::diagnostic::Severity;

    use super::*;

    /// A site whose entry program realizes one page per content source.
    ///
    /// Every file is written below the root; only files below `content` are sources, so a
    /// template elsewhere is readable without becoming a page of its own.
    fn site(files: &[(&str, &str)]) -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let toml = root.join("tola.toml");
        std::fs::write(&toml, "").unwrap();
        let configuration = Arc::new(
            load_site_config(
                Some(&toml),
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        );
        (directory, configuration)
    }

    /// A site that compiled, with the revision every question is asked against.
    fn compiled_site(
        files: &[(&str, &str)],
    ) -> (
        tempfile::TempDir,
        Arc<ResolvedSiteConfig>,
        BuildCancellation,
        SourceRevision,
    ) {
        let (directory, configuration) = site(files);
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
        incoming(
            &mut crate::analysis::DiskSources::default(),
            root,
            &SourceView::default(),
            None,
            &crate::sources::base_source_boundary(root, true),
            configuration,
            revision.checked().expect("the site compiles"),
            &source(root, name),
            cancellation,
        )
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
    fn page_incoming_names_no_caller() {
        let (_directory, configuration, cancellation, revision) =
            compiled_site(&[("content/one.typ", "One.\n")]);
        let found = incoming_for(&configuration, &revision, &cancellation, "content/one.typ");

        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn outgoing_names_reached_files() {
        let (_directory, configuration) = site(&[
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
    fn outgoing_skips_non_site_paths() {
        let (_directory, configuration) = site(&[(
            "content/document.typ",
            "#import \"@tola/host:0.0.0\": link\n#include \"../../outside.typ\"\n",
        )]);
        let root = configuration.get_root();
        let source = source(root, "content/document.typ");

        let found = outgoing(
            root,
            None,
            &crate::sources::base_source_boundary(root, true),
            &source,
        );

        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn prepare_names_the_written_source() {
        let (_directory, configuration) = site(&[(
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

    #[test]
    fn prepare_names_the_cursor_file() {
        let (_directory, configuration) = site(&[(
            "content/one.typ",
            "#include \"../templates/note.typ\"\nOne.\n",
        )]);
        let root = configuration.get_root();
        let source = source(root, "content/one.typ");
        let cursor = source.text().find("One.").unwrap();

        let item = prepare(
            root,
            None,
            &crate::sources::base_source_boundary(root, true),
            &source,
            cursor,
        )
        .expect("an item");

        assert_eq!(item.name, "content/one.typ");
        assert_eq!(item.kind, SymbolKind::FILE);
        assert_eq!(item.range, file_range());
        assert_eq!(
            item.data,
            Some(json!({"kind": "source", "path": "content/one.typ"}))
        );
    }
}
