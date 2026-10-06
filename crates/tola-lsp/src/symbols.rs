//! The protocol's symbols: one source's outline and labels, and the site's realized pages.

use lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, Location, OneOf, Position, Range, SymbolInformation,
    SymbolKind, Uri, WorkspaceSymbol,
};
use percent_encoding::percent_decode_str;
use tola_typst_syntax::outline::{self as outline_of, Declaration, DeclarationKind};
use tola_typst_syntax::typst_syntax::{Lines, LinkedNode, Source, SyntaxKind};

use crate::position;

/// The site's realized pages, as workspace symbols, for the ones whose route answers the search.
///
/// A page is named by the route it answers at, which is the name an author knows it by, and a
/// search reads that route both as the address encodes it and as the author writes it; its
/// container is the source that writes it, and its location is that source's start, because a page
/// has no declaration to select. A document no source of the site writes has nothing to open, so
/// it is not listed.
pub(super) fn pages(
    realized: &[crate::routes::Page],
    client_root: &crate::uri::ClientRoot,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    let query = query.to_lowercase();
    let root = client_root.resolved();
    realized
        .iter()
        .filter(|page| {
            let route = page.route.route.to_lowercase();
            route.contains(&query)
                || percent_decode_str(&route)
                    .decode_utf8_lossy()
                    .contains(&query)
        })
        .filter_map(|page| {
            let source = page.source?;
            let path = root.join(source.vpath().get_without_slash());
            Some(WorkspaceSymbol {
                name: page.route.route.clone(),
                kind: SymbolKind::FILE,
                tags: None,
                container_name: Some(source.vpath().get_without_slash().to_owned()),
                location: OneOf::Left(Location::new(
                    client_root.address(&path).ok()?,
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                )),
                data: None,
            })
        })
        .collect()
}

/// The source's declarations whose name answers the search, with protocol ranges and kinds.
pub(super) fn matching(
    source: &Source,
    query: &str,
) -> Vec<(String, SymbolKind, lsp_types::Range)> {
    outline_of::matching(source, query)
        .into_iter()
        .filter_map(|(name, kind, range)| {
            let range = position::utf16_range(source.lines(), range)?;
            Some((name, symbol_kind(kind), range))
        })
        .collect()
}

/// The labels the source declares, for the ones whose name answers the search.
///
/// A label is declared by `<name>` wherever it sits, and a search finds it by that name; the
/// element the label sits on is not read here.
pub(super) fn labels(source: &Source, query: &str) -> Vec<(String, lsp_types::Range)> {
    let query = query.to_lowercase();
    let mut found = Vec::new();
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::Label {
            let range = node.range();
            let name = &source.text()[range.start + 1..range.end - 1];
            if !name.is_empty()
                && name.to_lowercase().contains(&query)
                && let Some(range) =
                    position::utf16_range(source.lines(), range.start + 1..range.end - 1)
            {
                found.push((name.to_owned(), range));
            }
            continue;
        }
        stack.extend(node.children());
    }
    found.sort_by_key(|(_, range)| (range.start.line, range.start.character));
    found
}

pub(super) fn outline(
    source: &Source,
    uri: &Uri,
    hierarchical: bool,
) -> Option<DocumentSymbolResponse> {
    let symbols = outline_of::outline(source)?;
    if hierarchical {
        Some(DocumentSymbolResponse::Nested(
            symbols
                .into_iter()
                .filter_map(|symbol| document_symbol(symbol, source.lines()))
                .collect(),
        ))
    } else {
        let mut flat = Vec::new();
        flatten(symbols, source.lines(), uri, None, &mut flat);
        Some(DocumentSymbolResponse::Flat(flat))
    }
}

#[expect(
    deprecated,
    reason = "SymbolInformation retains deprecated for older clients"
)]
fn flatten(
    symbols: Vec<Declaration>,
    lines: &Lines<String>,
    uri: &Uri,
    container: Option<&str>,
    flat: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        let Some(range) = position::utf16_range(lines, symbol.name_range) else {
            continue;
        };
        let parent = (!symbol.children.is_empty()).then(|| symbol.name.clone());
        flat.push(SymbolInformation {
            name: symbol.name,
            kind: symbol_kind(symbol.kind),
            tags: None,
            deprecated: None,
            location: Location::new(uri.clone(), range),
            container_name: container.map(str::to_owned),
        });
        flatten(symbol.children, lines, uri, parent.as_deref(), flat);
    }
}

#[expect(
    deprecated,
    reason = "the protocol keeps `deprecated` for clients that predate symbol tags"
)]
fn document_symbol(declaration: Declaration, lines: &Lines<String>) -> Option<DocumentSymbol> {
    Some(DocumentSymbol {
        name: declaration.name,
        detail: declaration.detail,
        kind: symbol_kind(declaration.kind),
        tags: None,
        deprecated: None,
        range: position::utf16_range(lines, declaration.range)?,
        selection_range: position::utf16_range(lines, declaration.name_range)?,
        children: Some(
            declaration
                .children
                .into_iter()
                .filter_map(|child| document_symbol(child, lines))
                .collect(),
        ),
    })
}

fn symbol_kind(kind: DeclarationKind) -> SymbolKind {
    match kind {
        DeclarationKind::Namespace => SymbolKind::NAMESPACE,
        DeclarationKind::Function => SymbolKind::FUNCTION,
        DeclarationKind::Variable => SymbolKind::VARIABLE,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::protocol::Route;
    use crate::routes::Page;

    /// The page `name` is served at `route`, with the file written below `root`.
    fn page(root: &Path, name: &str, route: &str) -> Page {
        let source = root.join(name);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "Body\n").unwrap();
        Page {
            source: crate::identity::path_id(&source, root),
            route: Route {
                output: format!("{}/index.html", route.trim_matches('/')),
                route: route.to_owned(),
                url: None,
            },
        }
    }

    /// A page's address keeps the root spelling the editor named, not the resolved one.
    #[test]
    fn page_location_keeps_client_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let resolved = directory.path().canonicalize().unwrap();
        let spelled = directory.path().join("client-site");
        let served = page(&resolved, "content/document.typ", "/document/");

        let found = pages(
            &[served],
            &crate::uri::ClientRoot::with_resolved(&spelled, &resolved),
            "/document/",
        );

        assert_eq!(
            found.first().map(|symbol| symbol.location.clone()),
            Some(OneOf::Left(Location::new(
                crate::uri::from_file_path(&spelled.join("content/document.typ")).unwrap(),
                Range::new(Position::new(0, 0), Position::new(0, 0)),
            )))
        );
    }

    /// A search finds a page by the route its author knows, however the address encodes it.
    #[test]
    fn page_search_matches_decoded_route() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let served = page(&root, "content/café.typ", "/caf%C3%A9/");

        let found = pages(&[served], &crate::uri::ClientRoot::new(&root), "café");

        assert_eq!(found.len(), 1, "{found:?}");
    }

    /// A label answers a search by its own name, whatever element has it.
    #[test]
    fn label_answers_its_name() {
        let source = Source::detached("= Heading <intro>\n\n#figure([Body]) <fig-one>\n");
        let found = labels(&source, "fig");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "fig-one");
    }
}
