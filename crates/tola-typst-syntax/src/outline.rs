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
///
/// A workspace search reads the same outline a document outline does, so the two never disagree
/// about what a source declares.
pub fn matching(source: &Source, query: &str) -> Vec<(String, DeclarationKind, Range<usize>)> {
    let Some(symbols) = outline(source) else {
        return Vec::new();
    };
    let query = query.to_lowercase();
    let mut found = Vec::new();
    let mut stack = symbols;
    while let Some(declaration) = stack.pop() {
        if declaration.name.to_lowercase().contains(&query) {
            found.push((
                declaration.name.clone(),
                declaration.kind,
                declaration.name_range,
            ));
        }
        stack.extend(declaration.children);
    }
    found.sort_by_key(|(_, _, range)| range.start);
    found
}

/// The outline of the source, or `None` when it declares nothing to list.
///
/// A heading owns the bindings written inside its section, so the outline reads in document
/// order: a heading lists what its body declares before the next heading of its own level.
pub fn outline(source: &Source) -> Option<Vec<Declaration>> {
    let sections = sections::sections(source);
    let bindings = bindings(source);
    if sections.is_empty() && bindings.is_empty() {
        return None;
    }

    let mut declared = Vec::with_capacity(sections.len() + bindings.len());
    declared.extend(
        sections
            .iter()
            .enumerate()
            .map(|(index, section)| (section.heading.start, Pending::Heading(index))),
    );
    declared.extend(
        bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| (binding.range.start, Pending::Binding(index))),
    );
    declared.sort_by_key(|(offset, _)| *offset);

    let mut outline = Outline::default();
    for (_, declaration) in declared {
        match declaration {
            Pending::Heading(index) => {
                let section = &sections[index];
                outline.enter_heading(section.depth, heading_declaration(section)?);
            }
            Pending::Binding(index) => outline.place(binding_declaration(&bindings[index])?),
        }
    }
    Some(outline.into_declarations())
}

/// One declaration of the source, by its position in `sections` or in the source's bindings.
/// The two things an outline merges, in document order.
enum Pending {
    Heading(usize),
    Binding(usize),
}

/// The symbols of the outline, nested as they are built.
///
/// A heading stays open until a heading of the same or a higher level closes it, so the heading
/// a declaration belongs to is always the innermost one still open.
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
    fn enter_heading(&mut self, depth: usize, declaration: Declaration) {
        while self.open.last().is_some_and(|open| open.depth >= depth) {
            self.close_heading();
        }
        self.open.push(OpenHeading { depth, declaration });
    }

    /// Place one declaration under the heading that owns it, or at the outline's top level.
    fn place(&mut self, declaration: Declaration) {
        match self.open.last_mut() {
            Some(open) => open.declaration.children.push(declaration),
            None => self.roots.push(declaration),
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

/// A binding the source declares, with the range of its own name.
struct Binding {
    name: String,
    kind: DeclarationKind,
    range: Range<usize>,
}

/// A heading declaration spans its whole section; its name is the heading's own text.
fn heading_declaration(section: &Section) -> Option<Declaration> {
    Some(Declaration {
        name: section.name.clone(),
        detail: None,
        kind: DeclarationKind::Namespace,
        range: section.body.clone(),
        name_range: section.heading.clone(),
        children: Vec::new(),
    })
}

fn binding_declaration(binding: &Binding) -> Option<Declaration> {
    Some(Declaration {
        name: binding.name.clone(),
        detail: None,
        kind: binding.kind,
        range: binding.range.clone(),
        name_range: binding.range.clone(),
        children: Vec::new(),
    })
}

/// Every binding the source declares, in document order.
fn bindings(source: &Source) -> Vec<Binding> {
    let mut bindings = Vec::new();
    collect_bindings(&LinkedNode::new(source.root()), &mut bindings);
    bindings
}

fn collect_bindings(node: &LinkedNode<'_>, bindings: &mut Vec<Binding>) {
    if let Some(declared) = node.cast::<ast::LetBinding>() {
        let kind = match declared.kind() {
            ast::LetBindingKind::Closure(_) => DeclarationKind::Function,
            ast::LetBindingKind::Normal(_) => DeclarationKind::Variable,
        };
        for name in declared.kind().bindings() {
            if let Some(ident) = node.find(name.span()) {
                bindings.push(Binding {
                    name: name.get().to_string(),
                    kind,
                    range: ident.range(),
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

    /// A search reads the same outline a document outline shows, filtered by name.
    #[test]
    fn matching_symbols_filter_the_outline() {
        let source = Source::detached("= Second page\n\n#let marker = 1\n\n#let other = 2\n");
        let found: Vec<(String, DeclarationKind)> = matching(&source, "mark")
            .into_iter()
            .map(|(name, kind, _)| (name, kind))
            .collect();
        assert_eq!(found, [("marker".to_owned(), DeclarationKind::Variable)]);
        let headings: Vec<String> = matching(&source, "second")
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(headings.len(), 1, "the heading matches: {headings:?}");
        assert!(headings[0].contains("Second"));
        assert!(matching(&source, "absent").is_empty());
    }

    fn rendered(text: &str) -> Vec<(usize, String, DeclarationKind)> {
        fn walk(
            symbols: &[Declaration],
            depth: usize,
            rows: &mut Vec<(usize, String, DeclarationKind)>,
        ) {
            for declaration in symbols {
                rows.push((depth, declaration.name.clone(), declaration.kind));
                walk(&declaration.children, depth + 1, rows);
            }
        }
        let Some(symbols) = outline(&Source::detached(text)) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        walk(&symbols, 0, &mut rows);
        rows
    }

    #[test]
    fn headings_nest_by_depth() {
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
    fn bindings_belong_to_the_open_heading() {
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

    #[test]
    fn every_bound_name_lists() {
        assert_eq!(
            rendered("#let (a, b) = (1, 2)\n#let f(x) = x\n"),
            [
                (0, "a".to_owned(), DeclarationKind::Variable),
                (0, "b".to_owned(), DeclarationKind::Variable),
                (0, "f".to_owned(), DeclarationKind::Function),
            ]
        );
        assert_eq!(
            rendered("#let f(a) = {\n  let g(a) = a\n  let t = 1\n  g\n}\n#let t = 2\n"),
            [
                (0, "f".to_owned(), DeclarationKind::Function),
                (0, "g".to_owned(), DeclarationKind::Function),
                (0, "t".to_owned(), DeclarationKind::Variable),
                (0, "t".to_owned(), DeclarationKind::Variable),
            ]
        );
        assert_eq!(
            rendered(
                "// A comment\n#let after_line = 0\n\n/* A block\n   comment */\n#let after_block = 1\n"
            ),
            [
                (0, "after_line".to_owned(), DeclarationKind::Variable),
                (0, "after_block".to_owned(), DeclarationKind::Variable),
            ]
        );
    }

    #[test]
    fn empty_source_has_no_outline() {
        assert!(outline(&Source::detached("body only\n")).is_none());
    }
}
