//! The outline of one source: its headings and the bindings they own.

use std::ops::Range;

use typst_syntax::ast::AstNode;
use typst_syntax::{LinkedNode, Source, ast};

use crate::sections::{self, Section};

/// What one declaration of a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationKind {
    /// A heading, which names the section it opens.
    Namespace,
    /// A function a binding carries.
    Function,
    /// A value a binding carries.
    Variable,
}

/// One declaration a source's outline carries.
#[derive(Clone, Debug)]
pub struct Declaration {
    /// The name the declaration spells.
    pub name: String,
    /// What the caller shows beside the name, when it shows any.
    pub detail: Option<String>,
    /// What the declaration is.
    pub kind: DeclarationKind,
    /// The bytes the declaration owns.
    pub range: Range<usize>,
    /// The bytes the declaration's own name occupies.
    pub name_range: Range<usize>,
    /// The declarations inside this one, in document order.
    pub children: Vec<Declaration>,
}

/// The symbols of the source whose name contains `query`.
pub fn matching(src: &Source, query: &str) -> Vec<(String, DeclarationKind, Range<usize>)> {
    let Some(syms) = outline(src) else {
        return Vec::new();
    };
    let query = query.to_lowercase();
    let mut found = Vec::new();
    let mut stack = syms;
    while let Some(decl) = stack.pop() {
        if decl.name.to_lowercase().contains(&query) {
            found.push((decl.name.clone(), decl.kind, decl.name_range));
        }
        stack.extend(decl.children);
    }
    found.sort_by_key(|(_, _, range)| range.start);
    found
}

/// The outline of the source, or `None` when it declares nothing to list.
pub fn outline(src: &Source) -> Option<Vec<Declaration>> {
    let sections = sections::sections(src);
    let bindings = bindings(src);
    if sections.is_empty() && bindings.is_empty() {
        return None;
    }

    let mut declared = Vec::with_capacity(sections.len() + bindings.len());
    declared.extend(
        sections
            .iter()
            .enumerate()
            .map(|(idx, section)| (section.heading.start, Pending::Heading(idx))),
    );
    declared.extend(
        bindings
            .iter()
            .enumerate()
            .map(|(idx, binding)| (binding.range.start, Pending::Binding(idx))),
    );
    declared.sort_by_key(|(offset, _)| *offset);

    let mut outline = Outline::default();
    for (_, decl) in declared {
        match decl {
            Pending::Heading(idx) => {
                let section = &sections[idx];
                outline.enter_heading(section.depth, heading_declaration(section));
            }
            Pending::Binding(idx) => outline.place(binding_declaration(&bindings[idx])),
        }
    }
    Some(outline.into_declarations())
}

enum Pending {
    Heading(usize),
    Binding(usize),
}

#[derive(Default)]
struct Outline {
    roots: Vec<Declaration>,
    open: Vec<OpenHeading>,
}

struct OpenHeading {
    depth: usize,
    declaration: Declaration,
}

impl Outline {
    fn enter_heading(&mut self, depth: usize, decl: Declaration) {
        while self.open.last().is_some_and(|open| open.depth >= depth) {
            self.close_heading();
        }
        self.open.push(OpenHeading {
            depth,
            declaration: decl,
        });
    }

    fn place(&mut self, decl: Declaration) {
        match self.open.last_mut() {
            Some(open) => open.declaration.children.push(decl),
            None => self.roots.push(decl),
        }
    }

    fn into_declarations(mut self) -> Vec<Declaration> {
        while !self.open.is_empty() {
            self.close_heading();
        }
        self.roots
    }

    fn close_heading(&mut self) {
        let open = self.open.pop().expect("an open heading");
        self.place(open.declaration);
    }
}

struct Binding {
    name: String,
    kind: DeclarationKind,
    range: Range<usize>,
    name_range: Range<usize>,
}

fn heading_declaration(section: &Section) -> Declaration {
    Declaration {
        name: section.name.clone(),
        detail: None,
        kind: DeclarationKind::Namespace,
        range: section.body.clone(),
        name_range: section.heading.clone(),
        children: Vec::new(),
    }
}

fn binding_declaration(binding: &Binding) -> Declaration {
    Declaration {
        name: binding.name.clone(),
        detail: None,
        kind: binding.kind,
        range: binding.range.clone(),
        name_range: binding.name_range.clone(),
        children: Vec::new(),
    }
}

fn bindings(src: &Source) -> Vec<Binding> {
    let mut bindings = Vec::new();
    collect_bindings(&LinkedNode::new(src.root()), &mut bindings);
    bindings
}

fn collect_bindings(node: &LinkedNode<'_>, bindings: &mut Vec<Binding>) {
    if let Some(decl) = node.cast::<ast::LetBinding>() {
        let kind = match decl.kind() {
            ast::LetBindingKind::Closure(_) => DeclarationKind::Function,
            ast::LetBindingKind::Normal(_) => DeclarationKind::Variable,
        };
        for name in decl.kind().bindings() {
            if let Some(ident) = node.find(name.span()) {
                bindings.push(Binding {
                    name: name.get().to_string(),
                    kind,
                    range: node.range(),
                    name_range: ident.range(),
                });
            }
        }
    }
    for child in node.children() {
        collect_bindings(&child, bindings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_matching_symbols() {
        let src = Source::detached("= Second page\n\n#let marker = 1\n\n#let other = 2\n");
        let found: Vec<(String, DeclarationKind)> = matching(&src, "mark")
            .into_iter()
            .map(|(name, kind, _)| (name, kind))
            .collect();
        assert_eq!(found, [("marker".to_owned(), DeclarationKind::Variable)]);
        let headings: Vec<String> = matching(&src, "second")
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(headings.len(), 1, "the heading matches: {headings:?}");
        assert!(headings[0].contains("Second"));
        assert!(matching(&src, "absent").is_empty());
    }

    fn rendered(text: &str) -> Vec<(usize, String, DeclarationKind)> {
        fn walk(
            syms: &[Declaration],
            depth: usize,
            rows: &mut Vec<(usize, String, DeclarationKind)>,
        ) {
            for decl in syms {
                rows.push((depth, decl.name.clone(), decl.kind));
                walk(&decl.children, depth + 1, rows);
            }
        }
        let Some(syms) = outline(&Source::detached(text)) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        walk(&syms, 0, &mut rows);
        rows
    }

    #[test]
    fn outline_heading_nesting() {
        assert_eq!(
            rendered("= One\nbody\n== Two\ndeep\n=== Three\n= Four\n"),
            [
                (0, "One".to_owned(), DeclarationKind::Namespace),
                (1, "Two".to_owned(), DeclarationKind::Namespace),
                (2, "Three".to_owned(), DeclarationKind::Namespace),
                (0, "Four".to_owned(), DeclarationKind::Namespace),
            ]
        );
    }

    #[test]
    fn outline_heading_ownership() {
        assert_eq!(
            rendered("= One\n#let one = 1\n== Two\n#let two() = 2\n#let after = 3\n"),
            [
                (0, "One".to_owned(), DeclarationKind::Namespace),
                (1, "one".to_owned(), DeclarationKind::Variable),
                (1, "Two".to_owned(), DeclarationKind::Namespace),
                (2, "two".to_owned(), DeclarationKind::Function),
                (2, "after".to_owned(), DeclarationKind::Variable),
            ]
        );
        assert_eq!(
            rendered("#let shared = 1\n#let helper() = 2\n= One\n"),
            [
                (0, "shared".to_owned(), DeclarationKind::Variable),
                (0, "helper".to_owned(), DeclarationKind::Function),
                (0, "One".to_owned(), DeclarationKind::Namespace),
            ]
        );
    }
}
