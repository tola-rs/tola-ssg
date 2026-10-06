//! The name graph's answers, with every import identity the query needs proven by the compiler,
//! and the source-only replies that need no compilation.

use std::collections::{HashMap, hash_map};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceCompilation, SourceDiagnosticSession};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::{FileId, LinkedNode, Side, Source, SyntaxKind, VirtualRoot};
use tola_typst_syntax::imports::PendingImport;

use crate::packages::PackageAccess;
use crate::protocol::{SourceQuery, SourceReply};
use crate::sources::SourceView;
use tola_typst::typst::World;

use super::colors;
use super::semantic::Semantic;

/// The name graph's answer, with every import identity the query needs proven by the compiler.
///
/// A statement the compiler cannot prove — an unreadable source, a value that names no file, a
/// truncated trace, or two different files — stays unproven, and the graph answers only what it
/// established rather than guessing a cross-file identity.
#[expect(
    clippy::too_many_arguments,
    reason = "a graph reply reads session, package access, overrides, source, query, disk, and cancellation"
)]
pub(super) fn graph_reply(
    session: &mut SourceDiagnosticSession,
    config: &ResolvedSiteConfig,
    package_access: &PackageAccess<'_>,
    overrides: &[(PathBuf, Arc<str>)],
    view: &SourceView,
    source: &Source,
    cursor: usize,
    query: &SourceQuery,
    disk: &mut crate::analysis::DiskSources,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<SourceReply>> {
    let root = config.get_root();
    let locations = Some(&package_access.locations);
    let include_dependents = matches!(query, SourceQuery::References(_) | SourceQuery::Rename(_));
    let mut proven: Vec<(PendingImport, FileId)> = Vec::new();
    let mut visited: Vec<PendingImport> = Vec::new();
    // One request proves many statements of the same file; the file compiles once for all of
    // them, since nothing here outlives the request.
    let mut compilations: HashMap<String, Option<SourceCompilation>> = HashMap::new();
    let mut names = crate::analysis::graph(
        root,
        view,
        source,
        locations,
        &package_access.boundary,
        include_dependents,
        &proven,
        disk,
        None,
        cancellation,
    )?;
    loop {
        let fresh: Vec<_> = crate::analysis::unproven(&names, source, cursor, query)
            .into_iter()
            .filter(|pending| !visited.contains(pending))
            .collect();
        if fresh.is_empty() {
            break;
        }
        visited.extend(fresh.iter().copied());
        let mut proven_any = false;
        for pending in fresh {
            cancellation.ensure_active()?;
            if let Some(import) = proven_import(
                session,
                &mut compilations,
                root,
                package_access,
                overrides,
                pending,
                cancellation,
            )? {
                proven.push(import);
                proven_any = true;
            }
        }
        if !proven_any {
            break;
        }
        names = crate::analysis::graph(
            root,
            view,
            source,
            locations,
            &package_access.boundary,
            include_dependents,
            &proven,
            disk,
            None,
            cancellation,
        )?;
    }
    crate::analysis::respond(
        &names,
        source,
        cursor,
        query,
        &client_root.spell(client_root.resolved()),
        locations,
        &package_access.boundary,
    )
}
/// The file the official compiler proves one import statement's source expression names.
///
/// Every value the official trace observed at that expression must normalize to one file: a
/// string resolves as the compiler resolves it, a module has its own file. Anything less is
/// no identity: a missing or failed trace, a value that names nothing, a second file, or a
/// statement whose evaluation this file does not own all leave the statement unproven.
fn proven_import(
    session: &mut SourceDiagnosticSession,
    compilations: &mut HashMap<String, Option<SourceCompilation>>,
    root: &Path,
    package_access: &PackageAccess<'_>,
    overrides: &[(PathBuf, Arc<str>)],
    pending: PendingImport,
    cancellation: &BuildCancellation,
) -> Result<Option<(PendingImport, FileId)>> {
    cancellation.ensure_active()?;
    if !matches!(pending.file.root(), VirtualRoot::Project) {
        return Ok(None);
    }
    let relative = pending.file.vpath().get_without_slash().to_owned();
    let compilation = match compilations.entry(relative.clone()) {
        hash_map::Entry::Occupied(entry) => entry.into_mut(),
        hash_map::Entry::Vacant(entry) => entry.insert(
            session
                .inspect_source(&relative, overrides.to_vec(), cancellation)?
                .into_checked(),
        ),
    };
    let Some(compilation) = compilation.as_ref() else {
        return Ok(None);
    };
    // `typst::trace` swallows a failed compilation and its sink keeps only the first
    // `Sink::MAX_VALUES` values, so a truncated or uncompleted evaluation observes less than the
    // whole set of targets and proves nothing.
    if compilation.introspector().is_none() {
        return Ok(None);
    }
    let world = compilation.world();
    let Ok(source) = world.source(pending.file) else {
        return Ok(None);
    };
    let names = tola_typst_syntax::names::SourceNames::new(source.clone());
    let Some(import) = names.imports().get(pending.statement) else {
        return Ok(None);
    };
    if !names.evaluated_here(import.scope) {
        return Ok(None);
    }
    let Some(node) = tola_typst_syntax::syntax::node_at_range(&source, &import.source_range) else {
        return Ok(None);
    };
    let values = Semantic::new(compilation, cancellation).trace(node.span())?;
    if values.is_empty() || values.len() >= tola_typst::typst::engine::Sink::MAX_VALUES {
        return Ok(None);
    }
    let resolver =
        crate::analysis::file_resolver(Some(&package_access.locations), &package_access.boundary);
    let mut target = None;
    for (value, _) in &values {
        let Some(file) = traced_import_target(pending.file, value, root, resolver.as_ref()) else {
            return Ok(None);
        };
        if target.is_some_and(|target| target != file) {
            return Ok(None);
        }
        target = Some(file);
    }
    Ok(target.map(|target| (pending, target)))
}

/// The file one traced import value names, resolved as the compiler resolves it.
fn traced_import_target(
    file: FileId,
    value: &Value,
    root: &Path,
    resolver: Option<&tola_typst::FileResolver>,
) -> Option<FileId> {
    match value {
        Value::Str(path) => crate::analysis::import_target(file, path, root, resolver),
        Value::Module(module) => module.file_id(),
        _ => None,
    }
}
pub(crate) fn source_reply(
    source: &Source,
    _cursor: usize,
    query: &SourceQuery,
) -> Result<Option<SourceReply>> {
    Ok(Some(match query {
        SourceQuery::DocumentColors(_) => {
            SourceReply::DocumentColors(Some(colors::document(source)?))
        }
        SourceQuery::ColorPresentations(params) => {
            SourceReply::ColorPresentations(Some(colors::presentations(params.color, params.range)))
        }
        _ => return Ok(None),
    }))
}
/// The identifier the cursor stands in, when it stands in one.
pub(super) fn bare_name(source: &Source, cursor: usize) -> Option<&str> {
    let root = LinkedNode::new(source.root());
    [Side::Before, Side::After]
        .into_iter()
        .find_map(|side| root.leaf_at(cursor, side))
        .and_then(|leaf| match leaf.kind() {
            SyntaxKind::Ident => Some(&source.text()[leaf.range()]),
            // The byte ends the space before a name, which is where a cursor in front of the
            // name sits; the name the cursor opens is the one the author means.
            SyntaxKind::Space => {
                let next = leaf.next_leaf()?;
                (next.kind() == SyntaxKind::Ident).then(|| &source.text()[next.range()])
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;
    use lsp_types::{GotoDefinitionResponse, PrepareRenameResponse};

    /// A dynamic import the compiler proves to a file gives the spelling the editor asks about an
    /// identity: the definition reaches the export, the highlight marks its use, and the rename
    /// has a name to prepare.
    #[test]
    fn proven_dynamic_import_answers_its_export() {
        let mut site = QuerySession::new();
        site.site
            .write("content/library.typ", "#let greet() = [hello]\n");
        let Some(GotoDefinitionResponse::Scalar(location)) =
            site.definition("#let base = \"library\"\n#import base + \".typ\": greet\n#gree|t\n")
        else {
            panic!("no definition behind a dynamic import");
        };
        assert!(
            location.uri.as_str().ends_with("content/library.typ"),
            "{location:?}"
        );

        let highlights = site
            .highlights("#let base = \"library\"\n#import base + \".typ\": *\n#gre|et\n")
            .expect("highlights behind a dynamic import");
        assert_eq!(highlights.len(), 1, "{highlights:?}");

        let Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) =
            site.prepare_rename("#let base = \"library\"\n#import base + \".typ\": *\n#gre|et\n")
        else {
            panic!("no rename preparation behind a dynamic import");
        };
        assert_eq!(placeholder, "greet");
    }

    /// A dynamic import the compiler cannot prove leaves the spelling unbound: neither the
    /// highlights nor the rename preparation answers.
    #[test]
    fn unprovable_import_answers_nothing() {
        let mut site = QuerySession::new();
        assert_eq!(
            site.highlights("#let base = 1\n#import base: *\n#gre|et\n"),
            None
        );
        assert_eq!(
            site.prepare_rename("#let base = 1\n#import base: *\n#gre|et\n"),
            None
        );
    }

    /// A statement inside a function body runs again at each of its call sites, so a rename that
    /// would have to edit one refuses instead of breaking the callers this file never observes.
    #[test]
    fn rename_refuses_function_body_import() {
        let mut site = QuerySession::new();
        site.site.write(
            "content/other.typ",
            "#let partial(name) = {\n  import name + \".typ\": render\n  render\n}\n#partial(\"document\")\n",
        );
        assert_eq!(
            site.rename_refusal("#let ren|der() = [Body]\n", "welcome"),
            "unknownImport"
        );
    }
}
