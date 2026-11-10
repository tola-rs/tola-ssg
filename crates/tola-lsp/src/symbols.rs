//! The protocol's symbols: one source's outline and labels, and the site's realized pages.

use lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, Location, OneOf, Position, Range, SymbolKind,
    WorkspaceSymbol,
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
    source_root: &std::path::Path,
    client_root: &crate::uri::ClientRoot,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    let query = query.to_lowercase();
    realized
        .iter()
        .filter(|page| {
            let route = page.route.route.to_lowercase();
            route.contains(&query)
                || percent_decode_str(&route)
                    .decode_utf8_lossy()
                    .to_lowercase()
                    .contains(&query)
        })
        .filter_map(|page| {
            let source = page.source?;
            Some(WorkspaceSymbol {
                name: page.route.route.clone(),
                kind: SymbolKind::FILE,
                tags: None,
                container_name: Some(source.vpath().get_without_slash().to_owned()),
                location: OneOf::Left(Location::new(
                    crate::identity::client_uri(source, source_root, client_root).ok()?,
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
    let first_break = position::first_break(source.text());
    outline_of::matching(source, query)
        .into_iter()
        .filter_map(|(name, kind, range)| {
            let range = position::utf16_range_indexed(source.lines(), range, first_break)?;
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
    let first_break = position::first_break(source.text());
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::Label {
            let range = node.range();
            let name = &source.text()[range.start + 1..range.end - 1];
            if !name.is_empty()
                && name.to_lowercase().contains(&query)
                && let Some(range) = position::utf16_range_indexed(
                    source.lines(),
                    range.start + 1..range.end - 1,
                    first_break,
                )
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

pub(super) fn outline(source: &Source) -> Option<DocumentSymbolResponse> {
    let symbols = outline_of::outline(source)?;
    let first_break = position::first_break(source.text());
    Some(DocumentSymbolResponse::Nested(
        symbols
            .into_iter()
            .filter_map(|symbol| document_symbol(symbol, source.lines(), first_break))
            .collect(),
    ))
}

#[expect(
    deprecated,
    reason = "lsp-types marks the field deprecated but requires it to construct DocumentSymbol"
)]
fn document_symbol(
    declaration: Declaration,
    lines: &Lines<String>,
    first_break: Option<usize>,
) -> Option<DocumentSymbol> {
    Some(DocumentSymbol {
        name: declaration.name,
        detail: declaration.detail,
        kind: symbol_kind(declaration.kind),
        tags: None,
        deprecated: None,
        range: position::utf16_range_indexed(lines, declaration.range, first_break)?,
        selection_range: position::utf16_range_indexed(lines, declaration.name_range, first_break)?,
        children: Some(
            declaration
                .children
                .into_iter()
                .filter_map(|child| document_symbol(child, lines, first_break))
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

    /// A search finds a page by the route its author knows, however the address encodes it.
    #[test]
    fn page_search_matches_decoded_route() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let served = page(&root, "content/café.typ", "/CAF%C3%89/");

        let found = pages(
            &[served],
            &root,
            &crate::uri::ClientRoot::new(&root),
            "café",
        );

        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn symbol_ranges_follow_protocol_lines() {
        let text = "#let first = 1\u{2028}#let greet(name) = {\n  [Hi #name]\n}\n";
        let source = Source::detached(text);
        let Some(DocumentSymbolResponse::Nested(symbols)) = outline(&source) else {
            panic!("hierarchical document symbols");
        };
        let greet = symbols
            .iter()
            .find(|symbol| symbol.name == "greet")
            .unwrap();
        assert_eq!(greet.selection_range.start.line, 0);
        assert_eq!(greet.range.end.line, 2);
        let start = position::byte_offset(source.lines(), greet.selection_range.start).unwrap();
        let end = position::byte_offset(source.lines(), greet.selection_range.end).unwrap();
        assert_eq!(&text[start..end], "greet");
    }
}
